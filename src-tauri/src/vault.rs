use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::sessions;
use crate::supabase::SupabaseStore;

const PAGE_LIMIT: u32 = 24;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VaultItem {
    pub id: String,
    pub kind: String,
    pub created_at: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub download_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultPage {
    pub items: Vec<VaultItem>,
    pub has_more: bool,
    pub next_offset: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDownloadFile {
    pub id: String,
    pub kind: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDownloadReport {
    pub directory: String,
    pub saved: u32,
    pub failed: Vec<String>,
}

pub async fn list_vault_media(
    app: &AppHandle,
    model_id: &str,
    offset: u32,
    kind: &str,
) -> Result<VaultPage, String> {
    let offset = offset.min(100_000);
    let type_query = match kind {
        "photo" => "&type=photo",
        "video" => "&type=video",
        _ => "",
    };
    let url = format!(
        "https://onlyfans.com/api2/v2/vault/media?limit={PAGE_LIMIT}&offset={offset}&field=recent&sort=desc&list=all{type_query}"
    );
    let body = sessions::fetch_onlyfans_json(app, model_id, &url).await?;
    parse_vault_page(&body, offset, PAGE_LIMIT)
}

pub async fn download_vault_media(
    app: &AppHandle,
    store: &SupabaseStore,
    model_id: &str,
    files: Vec<VaultDownloadFile>,
) -> Result<VaultDownloadReport, String> {
    if files.is_empty() {
        return Err("no hay archivos para descargar".to_owned());
    }
    if files.len() > 400 {
        return Err("selecciona como máximo 400 archivos por descarga".to_owned());
    }
    let model = store
        .get_model(model_id)
        .await
        .map_err(|err| err.to_string())?;
    let folder_name = folder_name(&model.username, &model.display_name, model_id);
    let parent = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Guardar fotos y videos del vault")
            .pick_folder()
    })
    .await
    .map_err(|err| err.to_string())?
    .ok_or_else(|| "descarga cancelada".to_owned())?;
    let directory = parent.join(&folder_name);
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|err| err.to_string())?;
    let total = files.len();
    let mut saved = 0_u32;
    let mut failed = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let name = file_name(&file.id, &file.kind, &file.url);
        let _ = app.emit(
            "vault-download-progress",
            serde_json::json!({
                "modelId": model_id,
                "done": index,
                "total": total,
                "file": name,
            }),
        );
        match save_file(&client, &directory, &name, &file.url).await {
            Ok(()) => saved += 1,
            Err(err) => failed.push(format!("{name}: {err}")),
        }
    }
    let _ = app.emit(
        "vault-download-progress",
        serde_json::json!({
            "modelId": model_id,
            "done": total,
            "total": total,
            "file": "",
        }),
    );
    Ok(VaultDownloadReport {
        directory: directory.display().to_string(),
        saved,
        failed,
    })
}

pub fn parse_vault_page(body: &str, offset: u32, limit: u32) -> Result<VaultPage, String> {
    let value: Value = serde_json::from_str(body).map_err(|err| err.to_string())?;
    let list = value
        .get("list")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default();
    let items = list.iter().filter_map(vault_item).collect::<Vec<_>>();
    let has_more = value
        .get("hasMore")
        .and_then(Value::as_bool)
        .unwrap_or(items.len() as u32 == limit);
    Ok(VaultPage {
        items,
        has_more,
        next_offset: offset.saturating_add(limit),
    })
}

fn vault_item(value: &Value) -> Option<VaultItem> {
    let object = value.as_object()?;
    let id = object.get("id").and_then(value_id)?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("photo")
        .to_ascii_lowercase();
    if kind != "photo" && kind != "video" && kind != "gif" {
        return None;
    }
    let created_at = object
        .get("createdAt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let best = best_file(object);
    Some(VaultItem {
        id,
        kind,
        created_at,
        width: best.as_ref().and_then(|file| file.width),
        height: best.as_ref().and_then(|file| file.height),
        download_url: best.map(|file| file.url),
    })
}

struct RankedFile {
    url: String,
    rank: u32,
    width: Option<u32>,
    height: Option<u32>,
}

fn best_file(object: &serde_json::Map<String, Value>) -> Option<RankedFile> {
    let video = object.get("type").and_then(Value::as_str) == Some("video");
    let mut best: Option<RankedFile> = None;
    if let Some(files) = object.get("files").and_then(Value::as_object) {
        for (key, value) in files {
            if let Some(rank) = file_rank(key) {
                consider(&mut best, value, rank, video);
            }
        }
    }
    if let Some(sources) = object.get("videoSources").or_else(|| object.get("video_sources")) {
        match sources {
            Value::Object(map) => {
                for (key, value) in map {
                    let rank = source_rank(key);
                    consider(&mut best, value, rank, video);
                }
            }
            Value::Array(items) => {
                for item in items {
                    let height = item.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                    consider(&mut best, item, height.max(1), video);
                }
            }
            _ => {}
        }
    }
    best
}

fn consider(best: &mut Option<RankedFile>, value: &Value, rank: u32, video: bool) {
    let url = match value {
        Value::String(text) => text.clone(),
        Value::Object(object) => object
            .get("url")
            .or_else(|| object.get("src"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => String::new(),
    };
    if url.is_empty() {
        if let Value::Object(object) = value {
            if let Some(sources) = object.get("sources").and_then(Value::as_array) {
                for source in sources {
                    consider(best, source, rank, video);
                }
            }
        }
        return;
    }
    if !downloadable(&url) || (video && image_file(&url)) {
        return;
    }
    let width = value.get("width").and_then(Value::as_u64).map(|item| item as u32);
    let height = value.get("height").and_then(Value::as_u64).map(|item| item as u32);
    let replace = best.as_ref().map(|current| rank > current.rank).unwrap_or(true);
    if replace {
        *best = Some(RankedFile {
            url,
            rank,
            width,
            height,
        });
    }
}

fn file_rank(key: &str) -> Option<u32> {
    match key.to_ascii_lowercase().as_str() {
        "source" | "original" => Some(1_000_000),
        "full" => Some(900_000),
        "preview" | "thumb" | "squarepreview" | "square_preview" => None,
        _ => None,
    }
}

fn source_rank(key: &str) -> u32 {
    match key.to_ascii_lowercase().as_str() {
        "source" | "original" => 1_000_000,
        other => other.parse::<u32>().unwrap_or(1),
    }
}

fn image_file(url: &str) -> bool {
    let path = tauri::Url::parse(url)
        .map(|parsed| parsed.path().to_ascii_lowercase())
        .unwrap_or_default();
    [".jpg", ".jpeg", ".png", ".webp", ".gif"].iter().any(|ext| path.ends_with(ext))
}

fn downloadable(url: &str) -> bool {
    let Ok(parsed) = tauri::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let host = parsed.host_str().unwrap_or("");
    if host != "onlyfans.com" && !host.ends_with(".onlyfans.com") {
        return false;
    }
    let path = parsed.path().to_ascii_lowercase();
    !path.contains(".m3u8") && !path.contains(".mpd")
}

fn value_id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn folder_name(username: &str, display_name: &str, model_id: &str) -> String {
    let raw = if !username.trim().is_empty() {
        username
    } else if !display_name.trim().is_empty() {
        display_name
    } else {
        return format!("modelo-{model_id}");
    };
    let cleaned: String = raw
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        format!("modelo-{model_id}")
    } else {
        format!("{cleaned}-vault")
    }
}

fn file_name(id: &str, kind: &str, url: &str) -> String {
    let id: String = id.chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_').take(40).collect();
    let id = if id.is_empty() { "archivo".to_owned() } else { id };
    let kind = if kind == "video" { "video" } else { "foto" };
    format!("{kind}-{id}.{}", extension(url, kind))
}

fn extension(url: &str, kind: &str) -> &'static str {
    let path = tauri::Url::parse(url).map(|parsed| parsed.path().to_owned()).unwrap_or_default();
    let lower = path.to_ascii_lowercase();
    for ext in ["jpg", "jpeg", "png", "webp", "gif", "mp4", "mov", "m4v", "webm"] {
        if lower.ends_with(&format!(".{ext}")) {
            return match ext {
                "jpeg" => "jpg",
                "m4v" => "mp4",
                other => other,
            };
        }
    }
    if kind == "video" { "mp4" } else { "jpg" }
}

async fn save_file(client: &reqwest::Client, directory: &Path, name: &str, url: &str) -> Result<(), String> {
    if !downloadable(url) {
        return Err("OnlyFans no entregó un archivo directo en la calidad original".to_owned());
    }
    let response = client
        .get(url)
        .header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("OnlyFans respondió {}", response.status()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if content_type.starts_with("text/") || content_type.contains("mpegurl") || content_type.contains("dash+xml") {
        return Err("OnlyFans no entregó el archivo original".to_owned());
    }
    let path = unique_path(directory, name);
    let mut file = tokio::fs::File::create(&path).await.map_err(|err| err.to_string())?;
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        file.write_all(&chunk).await.map_err(|err| err.to_string())?;
    }
    file.flush().await.map_err(|err| err.to_string())?;
    Ok(())
}

fn unique_path(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(name).file_stem().and_then(|value| value.to_str()).unwrap_or("archivo");
    let ext = Path::new(name).extension().and_then(|value| value.to_str()).unwrap_or("bin");
    for index in 2..10_000 {
        let next = directory.join(format!("{stem}-{index}.{ext}"));
        if !next.exists() {
            return next;
        }
    }
    directory.join(format!("{stem}-{}.{}", std::process::id(), ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn photo_prefers_source_over_full_and_ignores_thumbs() {
        let item = json!({
            "id": 9,
            "type": "photo",
            "createdAt": "2026-01-01T00:00:00Z",
            "files": {
                "thumb": { "url": "https://cdn2.onlyfans.com/thumb.jpg", "width": 150, "height": 150 },
                "preview": { "url": "https://cdn2.onlyfans.com/preview.jpg", "width": 400, "height": 400 },
                "full": { "url": "https://cdn2.onlyfans.com/full.jpg", "width": 1080, "height": 1920 },
                "source": { "url": "https://cdn2.onlyfans.com/source.jpg", "width": 3000, "height": 4000 }
            }
        });
        let parsed = vault_item(&item).unwrap();
        assert_eq!(parsed.download_url.as_deref(), Some("https://cdn2.onlyfans.com/source.jpg"));
        assert_eq!(parsed.width, Some(3000));
        assert_eq!(parsed.height, Some(4000));
    }

    #[test]
    fn video_prefers_the_highest_direct_source() {
        let item = json!({
            "id": "44",
            "type": "video",
            "videoSources": {
                "240": "https://cdn3.onlyfans.com/clip-240.mp4",
                "720": "https://cdn3.onlyfans.com/clip-720.mp4",
                "original": "https://cdn3.onlyfans.com/clip-original.mp4"
            }
        });
        let parsed = vault_item(&item).unwrap();
        assert_eq!(
            parsed.download_url.as_deref(),
            Some("https://cdn3.onlyfans.com/clip-original.mp4")
        );
    }

    #[test]
    fn stream_manifests_are_not_downloadable() {
        let item = json!({
            "id": 3,
            "type": "video",
            "videoSources": {
                "720": "https://cdn2.onlyfans.com/playlist.m3u8",
                "240": "https://cdn2.onlyfans.com/low.mp4"
            }
        });
        let parsed = vault_item(&item).unwrap();
        assert_eq!(parsed.download_url.as_deref(), Some("https://cdn2.onlyfans.com/low.mp4"));
    }

    #[test]
    fn video_skips_the_poster_and_keeps_the_mp4() {
        let item = json!({
            "id": 8,
            "type": "video",
            "files": { "full": { "url": "https://cdn2.onlyfans.com/poster.jpg" } },
            "videoSources": { "720": "https://cdn2.onlyfans.com/clip-720.mp4" }
        });
        let parsed = vault_item(&item).unwrap();
        assert_eq!(parsed.download_url.as_deref(), Some("https://cdn2.onlyfans.com/clip-720.mp4"));
    }

    #[test]
    fn page_keeps_only_photos_and_videos() {
        let body = json!({
            "hasMore": false,
            "list": [
                { "id": 1, "type": "photo", "files": { "full": { "url": "https://cdn2.onlyfans.com/a.jpg" } } },
                { "id": 2, "type": "audio", "files": { "full": { "url": "https://cdn2.onlyfans.com/a.mp3" } } }
            ]
        });
        let page = parse_vault_page(&body.to_string(), 0, 24).unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(!page.has_more);
        assert_eq!(page.next_offset, 24);
    }
}
