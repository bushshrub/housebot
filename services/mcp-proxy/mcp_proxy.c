/*
 * mcp-proxy: HTTP bridge for stdio-based MCP servers.
 * Spawns child processes, speaks JSON-RPC 2.0 over pipes, exposes HTTP on port 3006.
 *
 * Endpoints:
 *   GET  /health
 *   GET  /mcp/:server/tools
 *   POST /mcp/:server/call    body: {"tool":"name","args":{...}}
 *
 * Servers are configured via MCP_SERVERS env var:
 *   MCP_SERVERS=ddg:duckduckgo-mcp-server,jellyfin:jellyfin-mcp
 */

#define _POSIX_C_SOURCE 200809L
#include <arpa/inet.h>
#include <ctype.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>
#include <pthread.h>

#define MAX_SERVERS  8
#define MAX_NAME_LEN 64
#define BUF_SIZE     65536
#define SMALL_BUF    4096

/* ── JSON building helpers ─────────────────────────────────────────────────── */

static void json_escape(const char* s, char* out, size_t out_sz) {
    size_t j = 0;
    out[j++] = '"';
    for (size_t i = 0; s[i] && j + 4 < out_sz; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c == '"')      { out[j++] = '\\'; out[j++] = '"'; }
        else if (c == '\\') { out[j++] = '\\'; out[j++] = '\\'; }
        else if (c == '\n') { out[j++] = '\\'; out[j++] = 'n'; }
        else if (c == '\r') { out[j++] = '\\'; out[j++] = 'r'; }
        else if (c == '\t') { out[j++] = '\\'; out[j++] = 't'; }
        else                { out[j++] = (char)c; }
    }
    out[j++] = '"';
    out[j]   = '\0';
}

/* ── MCP server process ────────────────────────────────────────────────────── */

typedef struct {
    char   name[MAX_NAME_LEN];
    char   command[256];
    pid_t  pid;
    int    stdin_fd;   /* we write to the child's stdin */
    int    stdout_fd;  /* we read from the child's stdout */
    long   next_id;
    pthread_mutex_t lock;
    int    ready;
} McpServer;

static McpServer g_servers[MAX_SERVERS];
static int       g_nservers = 0;

static int mcp_write(McpServer* s, const char* msg) {
    size_t len = strlen(msg);
    ssize_t n  = write(s->stdin_fd, msg, len);
    if (n < 0) return -1;
    /* newline */
    write(s->stdin_fd, "\n", 1);
    return 0;
}

/* Read one newline-terminated line from the child stdout */
static int mcp_readline(McpServer* s, char* out, size_t out_sz) {
    size_t i = 0;
    while (i + 1 < out_sz) {
        char c;
        ssize_t n = read(s->stdout_fd, &c, 1);
        if (n <= 0) return -1;
        if (c == '\n') break;
        out[i++] = c;
    }
    out[i] = '\0';
    return (int)i;
}

/* Send a JSON-RPC request and return the "result" portion (static buf) */
static const char* mcp_request(McpServer* s, const char* method, const char* params_json) {
    static char req_buf[BUF_SIZE];
    static char resp_buf[BUF_SIZE];

    pthread_mutex_lock(&s->lock);
    long id = ++s->next_id;
    snprintf(req_buf, sizeof(req_buf),
             "{\"jsonrpc\":\"2.0\",\"id\":%ld,\"method\":\"%s\",\"params\":%s}",
             id, method, params_json);
    mcp_write(s, req_buf);

    /* read until we get a response with our id */
    while (1) {
        if (mcp_readline(s, resp_buf, sizeof(resp_buf)) < 0) {
            pthread_mutex_unlock(&s->lock);
            return NULL;
        }
        /* crude id check */
        char id_pat[32];
        snprintf(id_pat, sizeof(id_pat), "\"id\":%ld", id);
        if (strstr(resp_buf, id_pat)) break;
    }
    pthread_mutex_unlock(&s->lock);

    /* extract "result": ... — return pointer into resp_buf */
    char* r = strstr(resp_buf, "\"result\":");
    if (!r) return NULL;
    return r + strlen("\"result\":");
}

static int mcp_handshake(McpServer* s) {
    const char* params =
        "{\"protocolVersion\":\"2024-11-05\","
        "\"capabilities\":{},"
        "\"clientInfo\":{\"name\":\"house-chatbot\",\"version\":\"0.1.0\"}}";
    const char* res = mcp_request(s, "initialize", params);
    if (!res) return -1;
    /* send initialized notification */
    mcp_write(s, "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}");
    return 0;
}

static int spawn_server(McpServer* s) {
    int in_pipe[2], out_pipe[2];
    if (pipe(in_pipe) || pipe(out_pipe)) return -1;

    pid_t pid = fork();
    if (pid < 0) return -1;
    if (pid == 0) {
        /* child */
        dup2(in_pipe[0],  STDIN_FILENO);
        dup2(out_pipe[1], STDOUT_FILENO);
        close(in_pipe[0]); close(in_pipe[1]);
        close(out_pipe[0]); close(out_pipe[1]);
        execlp(s->command, s->command, (char*)NULL);
        _exit(1);
    }
    close(in_pipe[0]);
    close(out_pipe[1]);
    s->pid       = pid;
    s->stdin_fd  = in_pipe[1];
    s->stdout_fd = out_pipe[0];
    s->next_id   = 0;
    pthread_mutex_init(&s->lock, NULL);

    if (mcp_handshake(s) < 0) {
        fprintf(stderr, "mcp-proxy: handshake failed for %s\n", s->name);
        return -1;
    }
    s->ready = 1;
    printf("mcp-proxy: server '%s' ready\n", s->name);
    return 0;
}

/* Parse MCP_SERVERS=prefix:cmd,prefix2:cmd2 */
static void init_servers(void) {
    const char* env = getenv("MCP_SERVERS");
    if (!env || !*env) {
        /* default: try duckduckgo */
        strncpy(g_servers[0].name,    "ddg", MAX_NAME_LEN - 1);
        strncpy(g_servers[0].command, "duckduckgo-mcp-server", 255);
        g_nservers = 1;
        spawn_server(&g_servers[0]);
        return;
    }
    char buf[1024];
    strncpy(buf, env, sizeof(buf) - 1);
    char* tok = strtok(buf, ",");
    while (tok && g_nservers < MAX_SERVERS) {
        char* colon = strchr(tok, ':');
        if (colon) {
            *colon = '\0';
            strncpy(g_servers[g_nservers].name,    tok,     MAX_NAME_LEN - 1);
            strncpy(g_servers[g_nservers].command, colon+1, 255);
            spawn_server(&g_servers[g_nservers]);
            g_nservers++;
        }
        tok = strtok(NULL, ",");
    }
}

static McpServer* find_server(const char* name) {
    for (int i = 0; i < g_nservers; i++) {
        if (strcmp(g_servers[i].name, name) == 0 && g_servers[i].ready)
            return &g_servers[i];
    }
    return NULL;
}

/* ── HTTP server ───────────────────────────────────────────────────────────── */

static ssize_t read_full(int fd, char* buf, size_t sz) {
    size_t got = 0;
    while (got < sz) {
        ssize_t n = read(fd, buf + got, sz - got);
        if (n <= 0) break;
        got += (size_t)n;
        if (memchr(buf, '\0', got) == NULL && got >= 4 &&
            memcmp(buf + got - 4, "\r\n\r\n", 4) == 0) break;
    }
    return (ssize_t)got;
}

static void send_http(int fd, int status, const char* body) {
    char hdr[256];
    snprintf(hdr, sizeof(hdr),
             "HTTP/1.1 %d %s\r\nContent-Type: application/json\r\n"
             "Content-Length: %zu\r\nConnection: close\r\n\r\n",
             status, status == 200 ? "OK" : "Error", strlen(body));
    write(fd, hdr, strlen(hdr));
    write(fd, body, strlen(body));
}

typedef struct { char method[8]; char path[256]; char body[BUF_SIZE]; } HttpReq;

static int parse_http(const char* raw, HttpReq* req) {
    /* method */
    const char* p = raw;
    char* wp = req->method;
    while (*p && *p != ' ' && (wp - req->method) < 7) *wp++ = *p++;
    *wp = '\0'; p++;
    /* path */
    wp = req->path;
    while (*p && *p != ' ' && *p != '\r' && (wp - req->path) < 255) *wp++ = *p++;
    *wp = '\0';
    /* body: after \r\n\r\n */
    const char* body_start = strstr(raw, "\r\n\r\n");
    if (body_start) {
        strncpy(req->body, body_start + 4, BUF_SIZE - 1);
    } else {
        req->body[0] = '\0';
    }
    return 0;
}

/* Extract a string field value from raw JSON (very naive) */
static int json_get_str(const char* json, const char* key, char* out, size_t out_sz) {
    char pat[128];
    snprintf(pat, sizeof(pat), "\"%s\":", key);
    const char* p = strstr(json, pat);
    if (!p) return -1;
    p += strlen(pat);
    while (*p == ' ') p++;
    if (*p != '"') return -1;
    p++;
    size_t i = 0;
    while (*p && *p != '"' && i + 1 < out_sz) {
        if (*p == '\\' && *(p+1)) { p++; }
        out[i++] = *p++;
    }
    out[i] = '\0';
    return 0;
}

/* Extract the args object from body JSON */
static int json_get_obj(const char* json, const char* key, char* out, size_t out_sz) {
    char pat[128];
    snprintf(pat, sizeof(pat), "\"%s\":", key);
    const char* p = strstr(json, pat);
    if (!p) return -1;
    p += strlen(pat);
    while (*p == ' ') p++;
    if (*p != '{') return -1;
    int depth = 0;
    size_t i = 0;
    while (*p && i + 1 < out_sz) {
        out[i++] = *p;
        if (*p == '{') depth++;
        else if (*p == '}') { depth--; if (depth == 0) { p++; break; } }
        p++;
    }
    out[i] = '\0';
    return 0;
}

static void handle_request(int client_fd) {
    static char raw[BUF_SIZE];
    ssize_t n = recv(client_fd, raw, sizeof(raw) - 1, 0);
    if (n <= 0) { close(client_fd); return; }
    raw[n] = '\0';

    /* read body if needed */
    char* cl_hdr = strstr(raw, "Content-Length:");
    if (!cl_hdr) cl_hdr = strstr(raw, "content-length:");
    if (cl_hdr) {
        int cl = atoi(cl_hdr + strlen("Content-Length:"));
        char* body_start = strstr(raw, "\r\n\r\n");
        if (body_start) {
            int have = (int)(n - (body_start + 4 - raw));
            while (have < cl) {
                ssize_t more = recv(client_fd, raw + n, sizeof(raw) - 1 - n, 0);
                if (more <= 0) break;
                n += more; raw[n] = '\0';
                have += (int)more;
            }
        }
    }

    HttpReq req;
    parse_http(raw, &req);

    /* GET /health */
    if (strcmp(req.method, "GET") == 0 && strcmp(req.path, "/health") == 0) {
        send_http(client_fd, 200, "{\"ok\":true}");
        close(client_fd); return;
    }

    /* parse /mcp/:server/tools or /mcp/:server/call */
    char server_name[MAX_NAME_LEN] = {0};
    char action[16] = {0};
    if (sscanf(req.path, "/mcp/%63[^/]/%15s", server_name, action) == 2) {
        McpServer* srv = find_server(server_name);
        if (!srv) {
            send_http(client_fd, 404, "{\"error\":\"server not found\"}");
            close(client_fd); return;
        }

        if (strcmp(req.method, "GET") == 0 && strcmp(action, "tools") == 0) {
            const char* result = mcp_request(srv, "tools/list", "{}");
            if (!result) {
                send_http(client_fd, 500, "{\"error\":\"mcp error\"}");
            } else {
                char resp[BUF_SIZE];
                snprintf(resp, sizeof(resp), "{\"result\":%s}", result);
                send_http(client_fd, 200, resp);
            }
            close(client_fd); return;
        }

        if (strcmp(req.method, "POST") == 0 && strcmp(action, "call") == 0) {
            char tool[256] = {0};
            char args[SMALL_BUF] = "{}";
            json_get_str(req.body, "tool", tool, sizeof(tool));
            json_get_obj(req.body, "args", args, sizeof(args));

            char params[BUF_SIZE];
            char esc_tool[512];
            json_escape(tool, esc_tool, sizeof(esc_tool));
            snprintf(params, sizeof(params),
                     "{\"name\":%s,\"arguments\":%s}", esc_tool, args);

            const char* result = mcp_request(srv, "tools/call", params);
            if (!result) {
                send_http(client_fd, 500, "{\"error\":\"mcp error\"}");
            } else {
                /* extract text content array */
                char resp[BUF_SIZE];
                snprintf(resp, sizeof(resp), "{\"result\":%s}", result);
                send_http(client_fd, 200, resp);
            }
            close(client_fd); return;
        }
    }

    send_http(client_fd, 404, "{\"error\":\"not found\"}");
    close(client_fd);
}

int main(void) {
    init_servers();

    int server_fd = socket(AF_INET, SOCK_STREAM, 0);
    int opt = 1;
    setsockopt(server_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family      = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port        = htons(3006);

    if (bind(server_fd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        perror("bind"); return 1;
    }
    listen(server_fd, 16);
    printf("mcp-proxy listening on port 3006\n");

    while (1) {
        int client_fd = accept(server_fd, NULL, NULL);
        if (client_fd < 0) continue;
        /* single-threaded: process one request at a time */
        handle_request(client_fd);
    }
    return 0;
}
