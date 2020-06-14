use base64;
use regex::Regex;
use reqwest::header;
use std::process::Command;
use std::str;
use std::{thread, time, cmp::Ordering};
use serde::{Deserialize, Serialize};

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
    Ok((port, token))
}

struct LCUClient {
    reqclient: reqwest::blocking::Client,
    port: u16,
}

fn build_lcu_client(port: u16, token: String) -> reqwest::Result<LCUClient> {
    let cert = b"-----BEGIN CERTIFICATE-----
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
    let cert = reqwest::Certificate::from_pem(cert).unwrap();

    let b64 = base64::encode(format!("riot:{}", token));
    let mut headers = header::HeaderMap::new();
    headers.insert(header::USER_AGENT, "LCU crate by DocWilco".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Basic {}", b64).parse().unwrap(),
    );
    println!("{} {:?}", port, token);
    let reqclient = reqwest::blocking::Client::builder()
        .add_root_certificate(cert)
        .default_headers(headers)
        .build()?;
    Ok(LCUClient {
        reqclient,
        port,
    })
}

impl LCUClient {
    fn get(&self, uri: &str) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.get(&url).send()
    }
    fn delete(&self, uri: &str) -> reqwest::Result<reqwest::blocking::Response> {
        let url = format!("https://127.0.0.1:{}{}", self.port, uri);
        self.reqclient.delete(&url).send()
    }
}

fn get_lcu_client() -> std::result::Result<LCUClient, String> {
    let mut result = get_lcu_info();
    while let Err(_) = result {
        println!("LCU not found, sleeping...");
        let ten_sec = time::Duration::from_secs(10);
        thread::sleep(ten_sec);
        result = get_lcu_info();
    }
    let (port, token) = result.unwrap();
    let result = build_lcu_client(port, token);
    match result {
        Ok(client) => Ok(client),
        Err(err) => Err(err.to_string()),
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RunePage {
    auto_modified_selections: Vec<serde_json::Value>,
    current: bool,
    id: u64,
    is_active: bool,
    is_deletable: bool,
    is_editable: bool,
    is_valid: bool,
    last_modified: u64,
    name: String,
    order: u32,
    primary_style_id: u64,
    selected_perk_ids: Vec<u64>,
    sub_style_id: u64,
}

fn main() -> Result<(), String> {
    let client = get_lcu_client()?;
    let pages = client.get("/lol-perks/v1/pages").unwrap().text().unwrap();
    let pages: Vec<RunePage> = serde_json::from_str(&pages).unwrap();
    let mut pages: Vec<RunePage> = pages.into_iter().filter(|page| page.is_deletable).collect();
    pages.sort_unstable_by(|a, b| {
        let ord = a.name.cmp(&b.name);
        if ord == Ordering::Equal {
            a.last_modified.cmp(&b.last_modified)
        } else {
            ord
        }
    });

    println!("all deletable pages:");
    for page in pages.iter() {
        println!("  {} [id:{}] [lm:{}]", page.name, page.id, page.last_modified);
    }

    let mut peekable = pages.into_iter().peekable();
    let mut pages_to_delete: Vec<RunePage> = Vec::new();
    while let Some(page) = peekable.next() {
        if let Some(next) = peekable.peek() {
            if next.name == page.name {
                pages_to_delete.push(page);
            }
        }
    }

    println!("deleting:");
    for page in pages_to_delete.into_iter() {
        println!("  {} [id:{}] [lm:{}]", page.name, page.id, page.last_modified);
        match client.delete(&format!("/lol-perks/v1/pages/{}", page.id)) {
            Ok(_) => { println!("    deleted"); },
            Err(_) => { return Err("error while deleting page".to_string()); }
        }
    }

    Ok(())
}
