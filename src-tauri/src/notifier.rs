use regex::Regex;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use serde_urlencoded;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerChanError {
    #[error("SENDKEY格式不正确")]
    InvalidKey,
    #[error("参数序列化失败: {0}")]
    Encode(#[from] serde_urlencoded::ser::Error),
    #[error("网络请求失败: {0}")]
    Request(#[from] reqwest::Error),
    #[error("正则表达式错误: {0}")]
    Regex(#[from] regex::Error),
}

pub async fn send_server_chan(sendkey: &str, text: &str, desp: &str) -> Result<(), ServerChanError> {
    let url = build_url(sendkey)?;
    let params = [("text", text), ("desp", desp)];
    let body = serde_urlencoded::to_string(params)?;
    let client = reqwest::Client::new();
    client
        .post(&url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(CONTENT_LENGTH, body.len() as u64)
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn build_url(key: &str) -> Result<String, ServerChanError> {
    let regex = Regex::new(r"sctp(\d+)t")?;
    if let Some(captures) = regex.captures(key) {
        let shard = &captures[1];
        Ok(format!("https://{}.push.ft07.com/send/{}.send", shard, key))
    } else {
        Err(ServerChanError::InvalidKey)
    }
}
