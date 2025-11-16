use std::env;
use regex::Regex;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use std::error::Error;

pub async fn sc_send(text: String, desp: String) -> Result<String, Box<dyn Error>> {
    let key = env::var("SENDKEY").unwrap();
    let params = [("text", text), ("desp", desp)];
    let post_data = serde_urlencoded::to_string(params)?;
    // 使用正则表达式提取 key 中的数字部分
    //Server酱3的key的前四个字符都是sctp
    let url = {
        let re = Regex::new(r"sctp(\d+)t")?;
        if let Some(captures) = re.captures(&key) {
            let num = &captures[1]; // 提取正则表达式捕获的数字部分
            format!("https://{}.push.ft07.com/send/{}.send", num, key)
        } else {
            return Err("Invalid sendkey format for sctp".into());
        }
    };
    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(CONTENT_LENGTH, post_data.len() as u64)
        .body(post_data)
        .send()
        .await?;
    let data = res.text().await?;
    Ok(data)
}
