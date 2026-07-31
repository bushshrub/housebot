//! Routing and the accept loop.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::assets;
use crate::http::{read_request, write_response, Request, Response};
use crate::nes::{palette, Nes};
use crate::roms::Shelf;
use crate::scores::{Board, Submission};

/// Frames a single request may ask the emulator to run.  The browser normally
/// asks for one; the cap keeps a request from monopolising the lock.
const MAX_FRAMES_PER_REQUEST: u32 = 4;

pub struct Arcade {
    board: Mutex<Board>,
    shelf: Shelf,
    console: Mutex<Option<Nes>>,
}

impl Arcade {
    pub async fn new(scores_path: impl Into<PathBuf>, roms_path: impl Into<PathBuf>) -> Self {
        Self {
            board: Mutex::new(Board::load(scores_path).await),
            shelf: Shelf::new(roms_path),
            console: Mutex::new(None),
        }
    }
}

pub async fn serve(
    addr: SocketAddr,
    scores_path: impl Into<PathBuf>,
    roms_path: impl Into<PathBuf>,
) -> anyhow::Result<()> {
    let arcade = Arc::new(Arcade::new(scores_path, roms_path).await);
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(
        addr = %listener.local_addr()?,
        roms = %arcade.shelf.directory().display(),
        "arcade open",
    );

    loop {
        let (mut stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                continue;
            }
        };
        let arcade = Arc::clone(&arcade);
        tokio::spawn(async move {
            // The emulator polls a frame at a time, so connections are reused
            // until the client stops sending or the exchange goes wrong.
            loop {
                let Some(request) = read_request(&mut stream).await else {
                    let _ =
                        write_response(&mut stream, &Response::error(400, "bad request"), false)
                            .await;
                    return;
                };
                let response = route(request, &arcade).await;
                if write_response(&mut stream, &response, true).await.is_err() {
                    tracing::debug!(%peer, "client went away");
                    return;
                }
            }
        });
    }
}

pub async fn route(request: Request, arcade: &Arcade) -> Response {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/" | "/index.html") => Response::html(assets::INDEX_HTML),
        ("GET", "/game.js") => Response::javascript(assets::GAME_JS),
        ("GET", "/machines.js") => Response::javascript(assets::MACHINES_JS),
        ("GET", "/favicon.ico") => Response::no_content(),
        ("GET", "/api/scores") => {
            let board = arcade.board.lock().await;
            match serde_json::to_vec(board.all()) {
                Ok(body) => Response::json(200, body),
                Err(_) => Response::error(500, "could not encode scores"),
            }
        }
        ("POST", "/api/scores") => submit_score(&request.body, arcade).await,
        ("GET", "/api/nes/cartridges") => {
            let titles = arcade.shelf.titles().await;
            Response::json(200, serde_json::json!({ "cartridges": titles }).to_string())
        }
        ("GET", "/api/nes/palette") => Response::json(
            200,
            serde_json::to_vec(&palette::PALETTE[..]).unwrap_or_default(),
        ),
        ("POST", "/api/nes/insert") => insert_cartridge(&request.body, arcade).await,
        ("POST", "/api/nes/frame") => run_frames(&request.body, arcade).await,
        ("GET" | "POST", _) => Response::error(404, "no such cabinet"),
        _ => Response::error(405, "method not allowed"),
    }
}

async fn submit_score(body: &[u8], arcade: &Arcade) -> Response {
    let submission: Submission = match serde_json::from_slice(body) {
        Ok(submission) => submission,
        Err(_) => return Response::error(400, "expected {cabinet, name, score}"),
    };
    let cabinet = submission.cabinet.clone();

    let mut board = arcade.board.lock().await;
    match board.submit(submission).await {
        Ok(rank) => Response::json(
            200,
            serde_json::json!({ "rank": rank, "scores": board.cabinet(&cabinet) }).to_string(),
        ),
        Err(rejected) => Response::error(400, rejected.message()),
    }
}

#[derive(Deserialize)]
struct InsertRequest {
    cartridge: String,
}

async fn insert_cartridge(body: &[u8], arcade: &Arcade) -> Response {
    let request: InsertRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(_) => return Response::error(400, "expected {cartridge}"),
    };

    let rom = match arcade.shelf.load(&request.cartridge).await {
        Ok(rom) => rom,
        Err(error) => return Response::error(404, &error),
    };
    match Nes::load(&rom) {
        Ok(console) => {
            *arcade.console.lock().await = Some(console);
            Response::json(
                200,
                serde_json::json!({ "cartridge": request.cartridge }).to_string(),
            )
        }
        Err(error) => Response::error(400, &error),
    }
}

#[derive(Deserialize)]
struct FrameRequest {
    #[serde(default)]
    buttons: u8,
    #[serde(default = "one")]
    frames: u32,
}

fn one() -> u32 {
    1
}

/// Advances the console and answers with raw palette indices — one byte per
/// pixel, which the browser colours through `/api/nes/palette`.
async fn run_frames(body: &[u8], arcade: &Arcade) -> Response {
    let request: FrameRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(_) => return Response::error(400, "expected {buttons, frames}"),
    };

    let mut console = arcade.console.lock().await;
    let Some(console) = console.as_mut() else {
        return Response::error(409, "no cartridge inserted");
    };
    console.set_buttons(0, request.buttons);
    for _ in 0..request.frames.clamp(1, MAX_FRAMES_PER_REQUEST) {
        console.run_frame();
    }
    Response::new(200, "application/octet-stream", console.frame().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nes::ppu::{HEIGHT, WIDTH};
    use crate::roms::BUILT_IN;
    use tempfile::TempDir;

    async fn arcade() -> (TempDir, Arcade) {
        let dir = TempDir::new().unwrap();
        let arcade = Arcade::new(dir.path().join("scores.json"), dir.path().join("roms")).await;
        (dir, arcade)
    }

    fn get(path: &str) -> Request {
        Request {
            method: "GET".into(),
            path: path.into(),
            body: Vec::new(),
        }
    }

    fn post(path: &str, body: &str) -> Request {
        Request {
            method: "POST".into(),
            path: path.into(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn serves_the_arcade_and_its_script() {
        let (_dir, arcade) = arcade().await;

        let page = route(get("/"), &arcade).await;
        assert_eq!(page.status, 200);
        assert!(String::from_utf8(page.body).unwrap().contains("<canvas"));

        let script = route(get("/game.js"), &arcade).await;
        assert_eq!(script.content_type, "text/javascript; charset=utf-8");
        assert!(String::from_utf8(script.body).unwrap().contains("webgl2"));

        assert_eq!(route(get("/favicon.ico"), &arcade).await.status, 204);
    }

    #[tokio::test]
    async fn scores_are_submitted_and_listed_per_cabinet() {
        let (_dir, arcade) = arcade().await;

        let response = route(
            post(
                "/api/scores",
                r#"{"cabinet":"snake","name":"ada","score":300}"#,
            ),
            &arcade,
        )
        .await;
        assert_eq!(response.status, 200);
        let payload: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(payload["rank"], 1);
        assert_eq!(payload["scores"][0]["name"], "ADA");

        let listed = route(get("/api/scores"), &arcade).await;
        let all: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(all["snake"], payload["scores"]);
    }

    #[tokio::test]
    async fn refuses_impossible_and_malformed_submissions() {
        let (_dir, arcade) = arcade().await;
        assert_eq!(
            route(
                post(
                    "/api/scores",
                    r#"{"cabinet":"snake","name":"x","score":50000}"#
                ),
                &arcade
            )
            .await
            .status,
            400
        );
        assert_eq!(route(post("/api/scores", "{"), &arcade).await.status, 400);
    }

    #[tokio::test]
    async fn the_console_boots_the_built_in_cartridge_and_returns_frames() {
        let (_dir, arcade) = arcade().await;

        let listed = route(get("/api/nes/cartridges"), &arcade).await;
        let payload: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(payload["cartridges"][0], BUILT_IN);

        assert_eq!(
            route(post("/api/nes/frame", "{}"), &arcade).await.status,
            409
        );

        let inserted = route(
            post(
                "/api/nes/insert",
                &format!(r#"{{"cartridge":"{BUILT_IN}"}}"#),
            ),
            &arcade,
        )
        .await;
        assert_eq!(inserted.status, 200);

        let frame = route(post("/api/nes/frame", r#"{"frames":4}"#), &arcade).await;
        assert_eq!(frame.content_type, "application/octet-stream");
        assert_eq!(frame.body.len(), WIDTH * HEIGHT);
        assert!(frame.body.iter().any(|&pixel| pixel != frame.body[0]));
    }

    #[tokio::test]
    async fn an_unknown_cartridge_is_not_loaded() {
        let (_dir, arcade) = arcade().await;
        let response = route(
            post("/api/nes/insert", r#"{"cartridge":"../../etc/passwd"}"#),
            &arcade,
        )
        .await;
        assert_eq!(response.status, 404);
    }

    #[tokio::test]
    async fn unknown_routes_and_methods_are_turned_away() {
        let (_dir, arcade) = arcade().await;
        assert_eq!(route(get("/../Cargo.toml"), &arcade).await.status, 404);
        assert_eq!(
            route(
                Request {
                    method: "DELETE".into(),
                    path: "/api/scores".into(),
                    body: Vec::new(),
                },
                &arcade
            )
            .await
            .status,
            405
        );
    }
}
