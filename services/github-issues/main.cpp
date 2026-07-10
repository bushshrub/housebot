#include <algorithm>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <memory>
#include <mutex>
#include <optional>
#include <sstream>
#include <string>
#include <vector>

#include <curl/curl.h>
#include <openssl/bio.h>
#include <openssl/evp.h>
#include <openssl/pem.h>
#include <openssl/rsa.h>
#include <nlohmann/json.hpp>

// ── tiny HTTP server from scratch (single-threaded, port 3005) ────────────────
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>

using json = nlohmann::json;

// ── helpers ───────────────────────────────────────────────────────────────────

static std::string env_or(const char* name, const char* fallback) {
    const char* v = std::getenv(name);
    return (v && *v) ? v : fallback;
}

static uint64_t unix_now() {
    using namespace std::chrono;
    return static_cast<uint64_t>(
        duration_cast<seconds>(system_clock::now().time_since_epoch()).count());
}

// ── base64url (no padding) ────────────────────────────────────────────────────

static std::string base64url_encode(const std::string& input) {
    BIO* b64 = BIO_new(BIO_f_base64());
    BIO* mem = BIO_new(BIO_s_mem());
    b64 = BIO_push(b64, mem);
    BIO_set_flags(b64, BIO_FLAGS_BASE64_NO_NL);
    BIO_write(b64, input.data(), static_cast<int>(input.size()));
    BIO_flush(b64);
    BUF_MEM* bptr;
    BIO_get_mem_ptr(b64, &bptr);
    std::string result(bptr->data, bptr->length);
    BIO_free_all(b64);
    // base64url: replace +/-> -_, strip padding
    std::replace(result.begin(), result.end(), '+', '-');
    std::replace(result.begin(), result.end(), '/', '_');
    while (!result.empty() && result.back() == '=') result.pop_back();
    return result;
}

// ── RS256 JWT ─────────────────────────────────────────────────────────────────

static std::string build_jwt(const std::string& app_id, const std::string& pem_key) {
    uint64_t now = unix_now();
    // header
    json header = {{"alg", "RS256"}, {"typ", "JWT"}};
    std::string h = base64url_encode(header.dump());
    // claims
    json claims = {
        {"iat", now - 60},
        {"exp", now + 600},
        {"iss", app_id}
    };
    std::string c = base64url_encode(claims.dump());
    std::string signing_input = h + "." + c;

    // sign with RSA private key
    BIO* bio = BIO_new_mem_buf(pem_key.c_str(), -1);
    EVP_PKEY* pkey = PEM_read_bio_PrivateKey(bio, nullptr, nullptr, nullptr);
    BIO_free(bio);
    if (!pkey) return "";

    EVP_MD_CTX* ctx = EVP_MD_CTX_new();
    EVP_DigestSignInit(ctx, nullptr, EVP_sha256(), nullptr, pkey);
    EVP_DigestSignUpdate(ctx, signing_input.c_str(), signing_input.size());
    size_t sig_len = 0;
    EVP_DigestSignFinal(ctx, nullptr, &sig_len);
    std::vector<unsigned char> sig(sig_len);
    EVP_DigestSignFinal(ctx, sig.data(), &sig_len);
    EVP_MD_CTX_free(ctx);
    EVP_PKEY_free(pkey);

    std::string sig_str(sig.begin(), sig.begin() + sig_len);
    return signing_input + "." + base64url_encode(sig_str);
}

// ── libcurl callback ──────────────────────────────────────────────────────────

static size_t write_cb(void* ptr, size_t size, size_t nmemb, std::string* out) {
    out->append(static_cast<char*>(ptr), size * nmemb);
    return size * nmemb;
}

static std::string http_post(const std::string& url, const std::string& bearer,
                             const std::string& body) {
    CURL* curl = curl_easy_init();
    std::string response;
    if (!curl) return response;

    struct curl_slist* headers = nullptr;
    headers = curl_slist_append(headers, ("Authorization: Bearer " + bearer).c_str());
    headers = curl_slist_append(headers, "Accept: application/vnd.github+json");
    headers = curl_slist_append(headers, "X-GitHub-Api-Version: 2022-11-28");
    headers = curl_slist_append(headers, "User-Agent: house-chatbot");
    headers = curl_slist_append(headers, "Content-Type: application/json");

    curl_easy_setopt(curl, CURLOPT_URL, url.c_str());
    curl_easy_setopt(curl, CURLOPT_HTTPHEADER, headers);
    curl_easy_setopt(curl, CURLOPT_POSTFIELDS, body.c_str());
    curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, write_cb);
    curl_easy_setopt(curl, CURLOPT_WRITEDATA, &response);
    curl_easy_setopt(curl, CURLOPT_SSL_VERIFYPEER, 1L);
    curl_easy_perform(curl);
    curl_slist_free_all(headers);
    curl_easy_cleanup(curl);
    return response;
}

// ── GitHub App credential cache ───────────────────────────────────────────────

struct Credentials {
    std::string app_id;
    std::string private_key;
    std::string installation_id;
    std::string repo;

    bool is_configured() const {
        return !app_id.empty() && !private_key.empty()
            && !installation_id.empty() && !repo.empty();
    }
};

struct TokenCache {
    std::string token;
    uint64_t    expires_at = 0;
    std::mutex  mtx;
};

static TokenCache g_cache;

static std::string get_installation_token(const Credentials& creds) {
    {
        std::lock_guard<std::mutex> lock(g_cache.mtx);
        if (!g_cache.token.empty() && unix_now() < g_cache.expires_at - 60) {
            return g_cache.token;
        }
    }
    std::string jwt = build_jwt(creds.app_id, creds.private_key);
    if (jwt.empty()) return "";

    std::string url = "https://api.github.com/app/installations/"
                    + creds.installation_id + "/access_tokens";
    std::string resp = http_post(url, jwt, "{}");
    try {
        auto j = json::parse(resp);
        std::string tok = j.at("token").get<std::string>();
        std::lock_guard<std::mutex> lock(g_cache.mtx);
        g_cache.token      = tok;
        g_cache.expires_at = unix_now() + 3600;
        return tok;
    } catch (...) {
        return "";
    }
}

static std::string create_github_issue(const Credentials& creds,
                                       const std::string& title,
                                       const std::string& body,
                                       const std::vector<std::string>& labels) {
    std::string tok = get_installation_token(creds);
    if (tok.empty()) return "";

    json payload = {{"title", title}, {"body", body}, {"labels", labels}};
    std::string url = "https://api.github.com/repos/" + creds.repo + "/issues";
    std::string resp = http_post(url, tok, payload.dump());
    try {
        return json::parse(resp).at("html_url").get<std::string>();
    } catch (...) {
        return "";
    }
}

// ── minimal HTTP/1.1 server ───────────────────────────────────────────────────

static std::string recv_request(int fd) {
    std::string buf;
    char tmp[4096];
    while (true) {
        ssize_t n = recv(fd, tmp, sizeof(tmp), 0);
        if (n <= 0) break;
        buf.append(tmp, n);
        if (buf.find("\r\n\r\n") != std::string::npos) break;
    }
    // read body if Content-Length present
    size_t hdr_end = buf.find("\r\n\r\n");
    if (hdr_end != std::string::npos) {
        size_t cl_pos = buf.find("Content-Length:");
        if (cl_pos == std::string::npos)
            cl_pos = buf.find("content-length:");
        if (cl_pos != std::string::npos) {
            size_t val_start = buf.find(':', cl_pos) + 1;
            size_t val_end   = buf.find("\r\n", val_start);
            int content_len  = std::stoi(buf.substr(val_start, val_end - val_start));
            size_t body_start = hdr_end + 4;
            while ((int)(buf.size() - body_start) < content_len) {
                ssize_t n = recv(fd, tmp, sizeof(tmp), 0);
                if (n <= 0) break;
                buf.append(tmp, n);
            }
        }
    }
    return buf;
}

static void send_response(int fd, int status, const std::string& body) {
    std::string resp = "HTTP/1.1 " + std::to_string(status) +
                       (status == 200 ? " OK" : " Error") + "\r\n"
                       "Content-Type: application/json\r\n"
                       "Content-Length: " + std::to_string(body.size()) + "\r\n"
                       "Connection: close\r\n\r\n" + body;
    send(fd, resp.c_str(), resp.size(), 0);
}

int main() {
    curl_global_init(CURL_GLOBAL_DEFAULT);

    Credentials creds;
    creds.app_id          = env_or("GITHUB_APP_ID", "");
    std::string raw_key   = env_or("GITHUB_APP_PRIVATE_KEY", "");
    // normalize literal \n
    std::string key;
    for (size_t i = 0; i < raw_key.size(); i++) {
        if (raw_key[i] == '\\' && i + 1 < raw_key.size() && raw_key[i+1] == 'n') {
            key += '\n'; i++;
        } else {
            key += raw_key[i];
        }
    }
    creds.private_key     = key;
    creds.installation_id = env_or("GITHUB_INSTALLATION_ID", "");
    creds.repo            = env_or("GITHUB_REPO", "");

    int port = 3005;
    int server_fd = socket(AF_INET, SOCK_STREAM, 0);
    int opt = 1;
    setsockopt(server_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    sockaddr_in addr{};
    addr.sin_family      = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port        = htons(port);
    bind(server_fd, (sockaddr*)&addr, sizeof(addr));
    listen(server_fd, 16);
    std::cout << "github-issues listening on port " << port << std::endl;

    while (true) {
        int client_fd = accept(server_fd, nullptr, nullptr);
        if (client_fd < 0) continue;

        std::string req = recv_request(client_fd);
        // parse method + path
        std::istringstream ss(req);
        std::string method, path, proto;
        ss >> method >> path >> proto;

        // GET /health
        if (method == "GET" && path == "/health") {
            send_response(client_fd, 200, R"({"ok":true})");
            close(client_fd);
            continue;
        }

        // POST /issues
        if (method == "POST" && path == "/issues") {
            size_t body_pos = req.find("\r\n\r\n");
            std::string body_str = (body_pos != std::string::npos)
                ? req.substr(body_pos + 4) : "";
            try {
                auto j = json::parse(body_str);
                std::string title = j.value("title", "");
                std::string body  = j.value("body", "");
                std::vector<std::string> labels = j.value("labels",
                    std::vector<std::string>{"bug"});

                if (!creds.is_configured()) {
                    send_response(client_fd, 200, R"({"url":null,"error":"GitHub App not configured"})");
                } else {
                    std::string url = create_github_issue(creds, title, body, labels);
                    json resp = {{"url", url.empty() ? json(nullptr) : json(url)}};
                    send_response(client_fd, 200, resp.dump());
                }
            } catch (...) {
                send_response(client_fd, 400, R"({"error":"invalid JSON"})");
            }
            close(client_fd);
            continue;
        }

        send_response(client_fd, 404, R"({"error":"not found"})");
        close(client_fd);
    }

    curl_global_cleanup();
    return 0;
}
