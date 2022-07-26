use std::cmp::min;
use native_tls::TlsConnector;
use regex::Regex;
use tungstenite::client::IntoClientRequest;
use std::collections::HashMap;
use std::net::TcpStream;
use std::process::Command;
use std::{thread, time};
use std::str;
use anyhow::Result;

static LCUCERT: &[u8; 1492] = b"-----BEGIN CERTIFICATE-----
MIIEIDCCAwgCCQDJC+QAdVx4UDANBgkqhkiG9w0BAQUFADCB0TELMAkGA1UEBhMC
VVMxEzARBgNVBAgTCkNhbGlmb3JuaWExFTATBgNVBAcTDFNhbnRhIE1vbmljYTET
MBEGA1UEChMKUmlvdCBHYW1lczEdMBsGA1UECxMUTG9MIEdhbWUgRW5naW5lZXJp
bmcxMzAxBgNVBAMTKkxvTCBHYW1lIEVuZ2luZWVyaW5nIENlcnRpZmljYXRlIEF1
dGhvcml0eTEtMCsGCSqGSIb3DQEJARYeZ2FtZXRlY2hub2xvZ2llc0ByaW90Z2Ft
ZXMuY29tMB4XDTEzMTIwNDAwNDgzOVoXDTQzMTEyNzAwNDgzOVowgdExCzAJBgNV
BAYTAlVTMRMwEQYDVQQIEwpDYWxpZm9ybmlhMRUwEwYDVQQHEwxTYW50YSBNb25p
Y2ExEzARBgNVBAoTClJpb3QgR2FtZXMxHTAbBgNVBAsTFExvTCBHYW1lIEVuZ2lu
ZWVyaW5nMTMwMQYDVQQDEypMb0wgR2FtZSBFbmdpbmVlcmluZyBDZXJ0aWZpY2F0
ZSBBdXRob3JpdHkxLTArBgkqhkiG9w0BCQEWHmdhbWV0ZWNobm9sb2dpZXNAcmlv
dGdhbWVzLmNvbTCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAKoJemF/
6PNG3GRJGbjzImTdOo1OJRDI7noRwJgDqkaJFkwv0X8aPUGbZSUzUO23cQcCgpYj
21ygzKu5dtCN2EcQVVpNtyPuM2V4eEGr1woodzALtufL3Nlyh6g5jKKuDIfeUBHv
JNyQf2h3Uha16lnrXmz9o9wsX/jf+jUAljBJqsMeACOpXfuZy+YKUCxSPOZaYTLC
y+0GQfiT431pJHBQlrXAUwzOmaJPQ7M6mLfsnpHibSkxUfMfHROaYCZ/sbWKl3lr
ZA9DbwaKKfS1Iw0ucAeDudyuqb4JntGU/W0aboKA0c3YB02mxAM4oDnqseuKV/CX
8SQAiaXnYotuNXMCAwEAATANBgkqhkiG9w0BAQUFAAOCAQEAf3KPmddqEqqC8iLs
lcd0euC4F5+USp9YsrZ3WuOzHqVxTtX3hR1scdlDXNvrsebQZUqwGdZGMS16ln3k
WObw7BbhU89tDNCN7Lt/IjT4MGRYRE+TmRc5EeIXxHkQ78bQqbmAI3GsW+7kJsoO
q3DdeE+M+BUJrhWorsAQCgUyZO166SAtKXKLIcxa+ddC49NvMQPJyzm3V+2b1roP
SvD2WV8gRYUnGmy/N0+u6ANq5EsbhZ548zZc+BI4upsWChTLyxt2RxR7+uGlS1+5
EcGfKZ+g024k/J32XP4hdho7WYAS2xMiV83CfLR/MNi8oSMaVQTdKD8cpgiWJk3L
XWehWA==
-----END CERTIFICATE-----";
fn get_lcu_info() -> Result<(u16, String), String> {
    let output = if cfg!(target_os = "windows") {
        Command::new("wmic")
            .args(&[
                "PROCESS",
                "WHERE",
                "name='LeagueClientUx.exe'",
                "GET",
                "commandline",
            ])
            .output()
            .expect("failed to execute process")
    } else {
        Command::new("sh")
            .arg("-c")
            .arg("ps -A | grep LeagueClientUx")
            .output()
            .expect("failed to execute process")
    };
    let output = str::from_utf8(&output.stdout).expect("not UTF8");
    let port_re = Regex::new(r"--app-port=([0-9]*)").unwrap();
    let token_re = Regex::new(r"--remoting-auth-token=([\w_-]*)").unwrap();
    let port_caps = port_re
        .captures(output)
        .ok_or_else(|| "No match for port".to_string())?;
    let token_caps = token_re
        .captures(output)
        .ok_or_else(|| "No match for token".to_string())?;
    let port: u16 = port_caps.get(1).unwrap().as_str().parse().unwrap();
    let token = token_caps.get(1).unwrap().as_str().to_string();
    println!("LCU: port={} token={}", port, token);
    Ok((port, token))
}

pub struct LCUClient {
    reqclient: reqwest::blocking::Client,
    port: u16,
}

fn build_lcu_client(port: u16, token: String) -> Result<LCUClient> {
    let cert = reqwest::Certificate::from_pem(LCUCERT).unwrap();

    let b64 = base64::encode(format!("riot:{}", token));
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        "LCU crate by DocWilco".parse().unwrap(),
    );
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Basic {}", b64).parse().unwrap(),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        "application/json".parse().unwrap(),
    );
    println!("{} {:?}", port, token);
    let reqclient = reqwest::blocking::Client::builder()
        .add_root_certificate(cert)
        .default_headers(headers)
        .build()?;
    Ok(LCUClient { reqclient, port })
}

impl LCUClient {
    pub fn new() -> Result<Self> {
        let mut result = get_lcu_info();
        while result.is_err() {
            println!("LCU not found, sleeping...");
            let ten_sec = time::Duration::from_secs(10);
            thread::sleep(ten_sec);
            result = get_lcu_info();
        }
        let (port, token) = result.unwrap();
        build_lcu_client(port, token)
    }

    pub fn get(&self, uri: &str) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.get(&url).send()
    }
    pub fn delete(&self, uri: &str) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.delete(&url).send()
    }
    pub fn patch<T: Into<reqwest::blocking::Body>>(&self, uri: &str, body: T) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.patch(&url).body(body).send()
    }
    pub fn post<T: Into<reqwest::blocking::Body>>(&self, uri: &str, body: T) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.post(&url).body(body).send()
    }
    pub fn put<T: Into<reqwest::blocking::Body>>(&self, uri: &str, body: T) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.put(&url).body(body).send()
    }
}

type BoxedCallback = Box<dyn FnMut(&serde_json::Value) -> Result<()>>;
struct Subscriber {
    callback: BoxedCallback,
    id: u64,
}

pub struct LCUWebSocket {
    next_id: u64,
    subscribers: HashMap<String, Vec<Subscriber>>,
    ws: tungstenite::protocol::WebSocket<native_tls::TlsStream<TcpStream>>,
}

impl LCUWebSocket {

    pub fn new() -> Self {
        let mut result = get_lcu_info();
        while result.is_err() {
            println!("LCU not found, sleeping...");
            let ten_sec = time::Duration::from_secs(10);
            thread::sleep(ten_sec);
            result = get_lcu_info();
        }
        let (port, token) = result.unwrap();

        let cert = native_tls::Certificate::from_pem(LCUCERT).unwrap();

        let connector = TlsConnector::builder()
            .add_root_certificate(cert)
            .build()
            .unwrap();

        let addr = format!("127.0.0.1:{}", port);
        let stream = TcpStream::connect(addr).unwrap();
        let stream = connector.connect("127.0.0.1", stream).unwrap();

        println!("got connection!");

        let mut request = "wss://127.0.0.1".into_client_request().unwrap();
        request.headers_mut().insert(tungstenite::http::header::USER_AGENT, "LCU crate by DocWilco".parse().unwrap());
        let b64 = base64::encode(format!("riot:{}", token));
        request.headers_mut().insert(tungstenite::http::header::AUTHORIZATION, format!("Basic {}", b64).parse().unwrap());
        let (ws, _) = tungstenite::client(request, stream).unwrap();
        LCUWebSocket{ws, subscribers: HashMap::new(), next_id: 0}
    }

    pub fn subscribe<C>(&mut self, event: String, callback: C) -> u64 
        where C: FnMut(&serde_json::Value) -> Result<()> + 'static {
        let message =
            tungstenite::protocol::Message::text(format!("[5, \"{}\"]", event));
        let id = self.next_id;
        let newsub = Subscriber{id, callback: Box::new(callback)};
        self.next_id += 1;
        self.subscribers.entry(event).or_default().push(newsub);
        self.ws.write_message(message).unwrap();
        id
    }

    pub fn unsubscribe(&mut self, handler_id: u64) -> Result<(), String> {
        for (_, handlers) in self.subscribers.iter_mut() {
            let len = handlers.len();
            handlers.retain(|h| h.id != handler_id);
            if handlers.len() != len {
                return Ok(());
            }
        }
        Err("handler id not found".to_string())
    }

    pub fn dispatch(&mut self) -> Result<(), String> {
        //println!("dispatch");
        let message = self.ws.read_message();
        if let Ok(tungstenite::protocol::Message::Text(message)) = message {
            if message.is_empty() {
                println!("empty message");
                return Ok(());
            }
            let message: serde_json::Value = serde_json::from_str(&message).unwrap();
            let event = &message[1];
            let event = event.as_str().unwrap().to_string();
            if let Some(subscribers) = self.subscribers.get_mut(&event) {
                let results: Vec<Result<()>> = subscribers.iter_mut().map(|sub| (sub.callback)(&message[2])).collect();
                for result in &results {
                    if let Err(err) = result {
                        return Err(err.to_string());
                    }
                }
            }
            Ok(())
        } else {
            let debug = format!("{:?}", message);
            println!("{:?}", &debug[..min(60, debug.len())]);
            Err("receiving failed".to_string())
        }
    }
}
