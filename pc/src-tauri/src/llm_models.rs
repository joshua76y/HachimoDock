/*
 * [Input] OpenAI 兼容服务的 base_url（如 http://127.0.0.1:9876/v1）与可选 API Key。
 * [Output] 该服务 `/models` 返回的模型 ID 列表。
 * [Pos] 设置页「获取模型列表」按钮的后端：走 Rust 侧请求，绕开 WebView 的
 *       CORS/CSP 限制，本地 OMLX 与云端服务共用。
 * [Sync] 与 ApiSettings 的「模型名称 / 接入点 ID」输入框配合使用。
 */

use std::time::Duration;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelList {
    pub models: Vec<String>,
    pub ms: u64,
}

fn endpoint(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.ends_with("/models") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/models")
    }
}

#[tauri::command]
pub fn list_openai_models(base_url: String, api_key: Option<String>) -> Result<ModelList, String> {
    let url = endpoint(&base_url);
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("请填写完整的服务地址，例如 http://127.0.0.1:9876/v1".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(6000))
        .build()
        .map_err(|e| format!("创建请求失败：{e}"))?;
    let started = std::time::Instant::now();
    let mut request = client.get(&url);
    if let Some(key) = api_key.filter(|value| !value.trim().is_empty()) {
        request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {}", key.trim()));
    }
    let response = request.send().map_err(|e| format!("请求 {url} 失败：{e}"))?;
    let status = response.status();
    let body = response.text().map_err(|e| format!("读取响应失败：{e}"))?;
    if !status.is_success() {
        /* 把服务端原文带回去：本地 OMLX 常见 401 "API key required"。 */
        return Err(format!("HTTP {}：{}", status.as_u16(), body.trim()));
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("响应不是合法 JSON：{e}"))?;
    let mut models: Vec<String> = value
        .get("data")
        .and_then(|data| data.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .map(|id| id.to_string())
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(format!("服务返回了空模型列表：{}", body.trim()));
    }
    Ok(ModelList {
        models,
        ms: started.elapsed().as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::endpoint;

    #[test]
    fn appends_models_once() {
        assert_eq!(endpoint("http://127.0.0.1:9876/v1"), "http://127.0.0.1:9876/v1/models");
        assert_eq!(endpoint("http://127.0.0.1:9876/v1/"), "http://127.0.0.1:9876/v1/models");
        assert_eq!(endpoint("http://127.0.0.1:9876/v1/models"), "http://127.0.0.1:9876/v1/models");
    }
}
