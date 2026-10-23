//! Loopback GitHub fake for Copilot device-login tests.
//!
//! Each route pops its scripted replies in order and repeats the last one, so
//! a test can script `authorization_pending` followed by a success on the same
//! polling path. Requests are recorded as `"METHOD /path"`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A scripted reply: HTTP status and JSON body.
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: &'static str,
}

pub(crate) fn ok(body: &'static str) -> Reply {
    Reply { status: 200, body }
}

/// A running fake that stops when dropped.
pub(crate) struct FakeGitHub {
    pub(crate) origin: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FakeGitHub {
    pub(crate) fn start(routes: Vec<(&'static str, Vec<Reply>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fake");
        listener.set_nonblocking(true).expect("nonblocking");
        let origin = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let routes = Arc::new(Mutex::new(routes));

        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        handle(stream, &routes, &thread_requests);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });

        Self {
            origin,
            requests,
            stop,
        }
    }

    /// Recorded requests, in arrival order, as `"METHOD /path"`.
    pub(crate) fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle(
    mut stream: std::net::TcpStream,
    routes: &Mutex<Vec<(&'static str, Vec<Reply>)>>,
    requests: &Mutex<Vec<String>>,
) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let Some((method, path, body_len)) = read_head(&mut stream) else {
        return;
    };
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body).ok();

    requests
        .lock()
        .expect("requests lock")
        .push(format!("{method} {path}"));

    let reply = {
        let mut routes = routes.lock().expect("routes lock");
        match routes.iter_mut().find(|(route, _)| *route == path) {
            Some((_, replies)) if replies.len() > 1 => replies.remove(0),
            Some((_, replies)) => replies.first().cloned().unwrap_or(Reply {
                status: 500,
                body: "{}",
            }),
            None => Reply {
                status: 404,
                body: "{}",
            },
        }
    };

    let response = format!(
        "HTTP/1.1 {} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    stream.write_all(response.as_bytes()).ok();
    stream.flush().ok();
}

/// Reads request line and headers; returns method, path, and body length.
fn read_head(stream: &mut std::net::TcpStream) -> Option<(String, String, usize)> {
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).ok()? == 0 {
            return None;
        }
        buffer.push(byte[0]);
        if buffer.len() > 64 * 1024 {
            return None;
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_string();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.split('?').next()?.to_string();
    let body_len = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    Some((method, path, body_len))
}
