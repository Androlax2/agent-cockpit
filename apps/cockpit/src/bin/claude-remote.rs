//! claude-remote: the phone page for the Claude sessions of this computer (cockpit::remote).
//!
//!     claude-remote serve     the page on 127.0.0.1:7380 (CLAUDE_REMOTE_ADDRESS), for `tailscale
//!                             serve` to publish on the tailnet only; refuses any other address
//!     claude-remote pair [name]   a one-time link and its QR code, to open on the phone (10 minutes)
//!     claude-remote phones        the paired phones: id, name, last seen
//!     claude-remote unpair [id]   forgets that phone, or every phone
//!
//! Run by claude-remote.service. Every request but /health and /pair/<code> needs a paired phone's secret (its
//! cookie); every POST also the X-Claude-Remote header, which another site cannot send.

use cockpit::remote;
use serde_json::{Value, json};
use std::io::Read;
use std::net::SocketAddr;
use std::process::Command;
use tiny_http::{Header, Method, Request, Response, Server};

const COOKIE: &str = "claude_remote";
const BODY_MAX: u64 = 16 * 1024;
const WORKERS: usize = 4;
const PAGE: &str = include_str!("../../assets/remote/page.html");
const SCRIPT: &str = include_str!("../../assets/remote/page.js");

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
}

fn respond(request: Request, status: u16, content_type: &str, body: String, extra: &[Header]) {
    let mut response = Response::from_string(body).with_status_code(status).with_header(header("Content-Type", content_type))
        .with_header(header("Cache-Control", "no-store"))
        .with_header(header("Referrer-Policy", "no-referrer"))
        .with_header(header("X-Content-Type-Options", "nosniff"))
        .with_header(header("Content-Security-Policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; form-action 'none'; base-uri 'none'"));
    for extra in extra {
        response = response.with_header(extra.clone());
    }
    let _ = request.respond(response);
}

fn respond_json(request: Request, status: u16, value: Value) {
    respond(request, status, "application/json", value.to_string(), &[]);
}

fn header_value(request: &Request, name: &str) -> Option<String> {
    request.headers().iter().find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|header| header.value.as_str().to_string())
}

fn cookie_secret(request: &Request) -> Option<String> {
    header_value(request, "Cookie")?.split(';').map(str::trim)
        .find_map(|pair| pair.strip_prefix(&format!("{COOKIE}=")).map(str::to_string))
}

fn is_session_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn body_json(request: &mut Request) -> Option<Value> {
    let mut body = String::new();
    request.as_reader().take(BODY_MAX).read_to_string(&mut body).ok()?;
    serde_json::from_str(&body).ok()
}

fn outcome(request: Request, result: Result<String, String>) {
    match result {
        Ok(message) => respond_json(request, 200, json!({"ok": true, "message": message})),
        Err(message) => respond_json(request, 409, json!({"ok": false, "message": message})),
    }
}

fn handle(mut request: Request) {
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let method = request.method().clone();
    // For the cockpit's Phone section: whether the server answers, nothing more.
    if method == Method::Get && path == "/health" {
        respond(request, 200, "text/plain", "ok".into(), &[]);
        return;
    }
    if let (Method::Get, Some(code)) = (&method, path.strip_prefix("/pair/")) {
        match remote::redeem(code, &header_value(&request, "User-Agent").unwrap_or_default()) {
            Some(secret) => respond(request, 303, "text/plain", "paired".into(), &[
                header("Set-Cookie", &format!("{COOKIE}={secret}; Path=/; Max-Age=31536000; Secure; HttpOnly; SameSite=Strict")),
                header("Location", "/")]),
            None => respond(request, 403, "text/plain", "This link is used or expired: run claude-remote pair again on the computer.".into(), &[]),
        }
        return;
    }
    if cookie_secret(&request).and_then(|secret| remote::paired_phone(&secret)).is_none() {
        respond(request, 401, "text/plain", "This phone is not paired: run claude-remote pair on the computer and open its link here.".into(), &[]);
        return;
    }
    if method == Method::Post && header_value(&request, "X-Claude-Remote").as_deref() != Some("1") {
        respond(request, 403, "text/plain", "refused: not from the page".into(), &[]);
        return;
    }
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (&method, parts.as_slice()) {
        (Method::Get, [""]) => respond(request, 200, "text/html; charset=utf-8", PAGE.into(), &[]),
        (Method::Get, ["page.js"]) => respond(request, 200, "text/javascript; charset=utf-8", SCRIPT.into(), &[]),
        (Method::Get, ["api", "state"]) => respond_json(request, 200, remote::state()),
        (Method::Get, ["api", "screen", id]) if is_session_id(id) => match remote::screen(id) {
            Ok(screen) => respond(request, 200, "text/plain; charset=utf-8", screen, &[]),
            Err(message) => respond(request, 409, "text/plain; charset=utf-8", message, &[]),
        },
        (Method::Post, ["api", "message", id]) if is_session_id(id) => {
            let id = id.to_string();
            let text = body_json(&mut request).and_then(|body| body.get("text").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
            outcome(request, remote::message(&id, &text));
        }
        (Method::Post, ["api", "answer", id]) if is_session_id(id) => {
            let id = id.to_string();
            match body_json(&mut request).and_then(|body| body.get("yes").and_then(Value::as_bool)) {
                Some(is_yes) => outcome(request, remote::answer(&id, is_yes)),
                None => respond_json(request, 400, json!({"ok": false, "message": "yes or no?"})),
            }
        }
        (Method::Post, ["api", "interrupt", id]) if is_session_id(id) => {
            let id = id.to_string();
            outcome(request, remote::interrupt(&id));
        }
        (Method::Post, ["api", "remote-control", id]) if is_session_id(id) => {
            let id = id.to_string();
            outcome(request, remote::toggle_remote_control(&id));
        }
        (Method::Post, ["api", "decide", id]) => {
            let id = id.to_string();
            let body = body_json(&mut request).unwrap_or(json!({}));
            match (body.get("run").and_then(Value::as_bool), body.get("command").and_then(Value::as_str)) {
                (Some(is_run), Some(command)) => {
                    let command = command.to_string();
                    outcome(request, remote::decide(&id, is_run, &command));
                }
                _ => respond_json(request, 400, json!({"ok": false, "message": "run or decline, with the command shown"})),
            }
        }
        _ => respond(request, 404, "text/plain", "not found".into(), &[]),
    }
}

fn serve() -> Result<(), String> {
    let address: SocketAddr = remote::address().parse()
        .map_err(|error| format!("not an address: {error}"))?;
    // Tailscale serve publishes it on the tailnet; bound anywhere else, the LAN would reach it.
    if !address.ip().is_loopback() {
        return Err(format!("refusing {address}: claude-remote listens on a loopback address only (tailscale serve publishes it)"));
    }
    let server = Server::http(address).map_err(|error| format!("cannot listen on {address}: {error}"))?;
    if let Ok(bound) = server.server_addr().to_ip().ok_or(()) {
        println!("claude-remote listening on {bound}");
    }
    // A few workers: a slow tmux or claude-sessions call for one phone does not stall the others.
    let server = std::sync::Arc::new(server);
    let workers: Vec<_> = (0..WORKERS).map(|_| {
        let server = server.clone();
        std::thread::spawn(move || {
            while let Ok(request) = server.recv() {
                handle(request);
            }
        })
    }).collect();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn pair(name: &str) -> Result<(), String> {
    let link = remote::pairing_link(name)?;
    println!("Open this on the phone within 10 minutes (once):\n\n  {link}\n");
    let _ = Command::new("qrencode").args(["-t", "ANSIUTF8", &link]).status();
    Ok(())
}

fn ago(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else { return "never".into() };
    let elapsed = cockpit::util::now_secs() - seconds;
    match elapsed {
        ..120 => "just now".into(),
        ..7200 => format!("{} min ago", elapsed / 60),
        ..172800 => format!("{} h ago", elapsed / 3600),
        _ => format!("{} days ago", elapsed / 86400),
    }
}

fn list_phones() {
    let phones = remote::phones();
    if phones.is_empty() {
        println!("No phone is paired: claude-remote pair [name].");
    }
    for phone in phones {
        println!("{}  {:<20} last seen {}", phone["id"].as_str().unwrap_or(""), phone["name"].as_str().unwrap_or(""), ago(phone["last_seen"].as_i64()));
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["serve"] => serve(),
        ["pair"] => pair(""),
        ["pair", name] => pair(name),
        ["phones"] => Ok(list_phones()),
        ["unpair"] => remote::unpair_all().map(|()| println!("Every phone forgotten.")).map_err(|error| error.to_string()),
        ["unpair", id] => remote::unpair(id).map(|name| println!("{name} forgotten.")),
        _ => Err("usage: claude-remote serve | pair [name] | phones | unpair [id]".into()),
    };
    if let Err(error) = result {
        eprintln!("claude-remote: {error}");
        std::process::exit(1);
    }
}
