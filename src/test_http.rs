//! Minimaler HTTP-Server für Tests: beantwortet genau eine Anfrage mit einer festen Antwort und gibt die
//! empfangene Anfrage zurück.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread::JoinHandle;

pub struct Request {
    pub head: String,
    pub body: String,
}

/// Startet den Server und liefert seine Basis-URL (`http://127.0.0.1:<port>`).
pub fn serve_once(status: u16, body: impl Into<String>) -> (String, JoinHandle<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let body = body.into();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            head.push_str(&line);
        }
        let len = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().to_string())
            })
            .map_or(0, |v| v.parse().unwrap());
        let mut body_bytes = vec![0; len];
        reader.read_exact(&mut body_bytes).unwrap();
        let response = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        reader.get_mut().write_all(response.as_bytes()).unwrap();
        Request {
            head,
            body: String::from_utf8(body_bytes).unwrap(),
        }
    });
    (url, handle)
}
