use crate::config::AppConfig;
use crate::ocr::OcrLineBox;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedLine {
    pub id: usize,
    pub text: String,
}

#[derive(Debug, Deserialize)]
struct TranslationResponseEnvelope {
    translations: Vec<TranslatedLine>,
}

/// Client for DeepSeek's Chat Completions API (`https://api.deepseek.com/chat/completions`).
/// Follows `https://api-docs.deepseek.com/guides/thinking_mode` by passing the top-level
/// `"thinking": { "type": "enabled" | "disabled" }` parameter directly on the configured model.
#[derive(Clone)]
pub struct DeepSeekClient {
    http: Client,
    config: AppConfig,
}

impl DeepSeekClient {
    pub fn new(config: AppConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    fn endpoint_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.deepseek_base_url.trim_end_matches('/')
        )
    }

    /// Mode A: Fast In-Place Overlay Translation.
    /// Translates `Windows.Media.Ocr` bounding-box lines in a single structured JSON call
    /// using the configured model with `"thinking": { "type": "disabled" }`.
    pub async fn translate_ocr_lines(
        &self,
        lines: &[OcrLineBox],
        target_lang: &str,
    ) -> Result<Vec<TranslatedLine>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }

        let api_key = AppConfig::get_deepseek_api_key()?;
        let url = self.endpoint_url();
        let model = self.config.deepseek_model.as_str();

        let input_lines: Vec<_> = lines
            .iter()
            .map(|l| json!({ "id": l.id, "text": l.text }))
            .collect();

        let payload = json!({
            "model": model,
            "thinking": {
                "type": "disabled"
            },
            "response_format": { "type": "json_object" },
            "temperature": 0.1,
            "messages": [
                {
                    "role": "system",
                    "content": format!(
                        "Translate each screen text line into {}. \
                         Preserve the exact line 'id' and keep translations concise so they fit the original visual bounding box. \
                         Respond strictly with JSON: {{\"translations\": [{{\"id\": 0, \"text\": \"...\"}}]}}",
                        target_lang
                    )
                },
                {
                    "role": "user",
                    "content": serde_json::to_string(&json!({
                        "target_lang": target_lang,
                        "lines": input_lines
                    })).unwrap_or_default()
                }
            ]
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&api_key)
            .json(&payload)
            .send()
            .await
            .context("Failed to send translation request to DeepSeek")?;

        let resp_json: serde_json::Value = resp
            .error_for_status()
            .context("DeepSeek API returned an error status")?
            .json()
            .await?;

        let raw_content = resp_json["choices"][0]["message"]["content"]
            .as_str()
            .context("Missing content in DeepSeek response")?;

        let cleaned = strip_json_fence(raw_content);
        let parsed: TranslationResponseEnvelope = serde_json::from_str(cleaned)
            .context("Failed to parse DeepSeek JSON translation response")?;

        Ok(parsed.translations)
    }

    /// Streams the response from DeepSeek via SSE using the configured model (`self.config.deepseek_model`)
    /// and the explicit `"thinking": { "type": "enabled" | "disabled" }` parameter per
    /// `https://api-docs.deepseek.com/guides/thinking_mode`.
    pub async fn ask_vision_stream(
        &self,
        webp_data_url: &str,
        local_ocr_context: Option<&str>,
        user_prompt: &str,
        mut on_delta: impl FnMut(&str),
    ) -> Result<()> {
        let api_key = AppConfig::get_deepseek_api_key()?;
        let url = self.endpoint_url();
        let model = self.config.deepseek_model.as_str();
        let thinking_type = if self.config.enable_ai_reasoning {
            "enabled"
        } else {
            "disabled"
        };

        let ocr_str = local_ocr_context.unwrap_or("").trim();
        let text_prompt = if !ocr_str.is_empty() {
            format!(
                "User Request: {}\n\n[Extracted Screen Text (Windows.Media.Ocr)]:\n{}",
                user_prompt, ocr_str
            )
        } else {
            format!("User Request: {}", user_prompt)
        };

        let is_vision_capable =
            model.contains("flash") || model.contains("v4") || model.contains("vl");

        let mut response = None;

        if is_vision_capable {
            let multimodal_payload = json!({
                "model": model,
                "stream": true,
                "thinking": {
                    "type": thinking_type
                },
                "messages": [
                    {
                        "role": "system",
                        "content": "You are WinLook, a concise Windows screen intelligence assistant. Respond in clean Markdown."
                    },
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": webp_data_url
                                }
                            },
                            {
                                "type": "text",
                                "text": text_prompt
                            }
                        ]
                    }
                ]
            });

            if let Ok(res) = self
                .http
                .post(&url)
                .bearer_auth(&api_key)
                .json(&multimodal_payload)
                .send()
                .await
            {
                if res.status().is_success() {
                    response = Some(res);
                }
            }
        }

        // Standard text+OCR request on the same configured model with `"thinking": { "type": ... }`
        let response = match response {
            Some(r) => r,
            None => {
                let text_payload = json!({
                    "model": model,
                    "stream": true,
                    "thinking": {
                        "type": thinking_type
                    },
                    "messages": [
                        {
                            "role": "system",
                            "content": "You are WinLook, a concise Windows screen intelligence assistant. Analyze the captured screen text and answer the user's request in clean Markdown."
                        },
                        {
                            "role": "user",
                            "content": if !ocr_str.is_empty() {
                                text_prompt.clone()
                            } else {
                                format!(
                                    "{}\n\n(No OCR text was detected in the selected screen region.)",
                                    user_prompt
                                )
                            }
                        }
                    ]
                });

                let res = self
                    .http
                    .post(&url)
                    .bearer_auth(&api_key)
                    .json(&text_payload)
                    .send()
                    .await
                    .context("Failed to connect to DeepSeek API")?;

                res.error_for_status()
                    .context("DeepSeek API returned an error status")?
            }
        };

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut in_reasoning = false;

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res.context("Error reading DeepSeek SSE stream")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer.drain(..=newline_pos);

                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let delta_obj = &val["choices"][0]["delta"];

                        // Stream `reasoning_content` when thinking mode is enabled
                        if let Some(reasoning) = delta_obj["reasoning_content"].as_str() {
                            if !reasoning.is_empty() {
                                if !in_reasoning {
                                    in_reasoning = true;
                                    on_delta("> *Thinking:* ");
                                }
                                on_delta(reasoning);
                            }
                        }

                        if let Some(content) = delta_obj["content"].as_str() {
                            if !content.is_empty() {
                                if in_reasoning {
                                    in_reasoning = false;
                                    on_delta("\n\n---\n\n");
                                }
                                on_delta(content);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

fn strip_json_fence(input: &str) -> &str {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_end_matches("```").trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_end_matches("```").trim()
    } else {
        trimmed
    }
}
