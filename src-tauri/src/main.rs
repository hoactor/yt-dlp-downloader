#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use regex::Regex;
use serde::Serialize;
use std::process::Stdio;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use std::fs;
use std::path::Path;

static CHILD_PID: Mutex<Option<u32>> = Mutex::new(None);

/// 영상 폴더에 원본 YouTube 링크 바로가기(.webloc) 생성
/// macOS Finder에서 더블클릭 시 Safari/Chrome에서 열림
fn create_webloc(folder: &Path, url: &str) {
    let webloc_path = folder.join("원본 링크.webloc");
    if webloc_path.exists() {
        return;
    }
    // macOS .webloc 표준 plist 포맷 (탭 대신 스페이스, LF 줄바꿈)
    let content = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n    <key>URL</key>\n    <string>{}</string>\n</dict>\n</plist>\n",
        url
    );
    let _ = fs::write(&webloc_path, content);

    // 추가로 .txt에도 URL 저장 (확실한 대체 수단)
    let txt_path = folder.join("원본 링크.txt");
    if !txt_path.exists() {
        let _ = fs::write(&txt_path, url);
    }
}

/// .vtt 파일에서 타임스탬프와 태그를 제거하고 순수 텍스트만 추출하여 .txt로 저장
fn vtt_to_txt(vtt_path: &Path) {
    let content = match fs::read_to_string(vtt_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let timestamp_re = Regex::new(r"^\d{2}:\d{2}:\d{2}\.\d{3}\s*-->.*$").unwrap();
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    let position_re = Regex::new(r"(?i)(WEBVTT|Kind:|Language:|\balign:|\bposition:)").unwrap();
    let note_re = Regex::new(r"^NOTE\b").unwrap();

    let mut lines_out: Vec<String> = Vec::new();
    let mut prev_line = String::new();
    let mut skip_note = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // NOTE 블록 스킵
        if note_re.is_match(trimmed) {
            skip_note = true;
            continue;
        }
        if skip_note {
            if trimmed.is_empty() {
                skip_note = false;
            }
            continue;
        }

        // 빈 줄, 타임스탬프, 메타데이터, 숫자만 있는 큐 인덱스 스킵
        if trimmed.is_empty()
            || timestamp_re.is_match(trimmed)
            || position_re.is_match(trimmed)
            || trimmed.parse::<u64>().is_ok()
        {
            continue;
        }

        // HTML 태그 제거
        let clean = tag_re.replace_all(trimmed, "").trim().to_string();
        if clean.is_empty() {
            continue;
        }

        // 중복 라인 제거 (vtt는 같은 줄이 반복됨)
        if clean != prev_line {
            lines_out.push(clean.clone());
            prev_line = clean;
        }
    }

    let txt_path = vtt_path.with_extension("txt");
    let _ = fs::write(&txt_path, lines_out.join("\n"));
}

fn find_ytdlp() -> Result<String, String> {
    let candidates = [
        "/opt/homebrew/bin/yt-dlp",
        "/usr/local/bin/yt-dlp",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    std::process::Command::new("which")
        .arg("yt-dlp")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| "yt-dlp가 설치되어 있지 않습니다. brew install yt-dlp 를 실행하세요.".to_string())
}

#[tauri::command]
async fn check_ytdlp() -> Result<String, String> {
    let bin = find_ytdlp()?;
    let output = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tauri::command]
async fn fetch_title(url: String) -> Result<String, String> {
    let bin = find_ytdlp()?;
    let output = std::process::Command::new(&bin)
        .args(["--get-title", &url])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn config_path() -> std::path::PathBuf {
    let mut p = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("yt-dlp-downloader");
    let _ = fs::create_dir_all(&p);
    p.push("settings.json");
    p
}

#[tauri::command]
async fn get_default_download_dir() -> Result<String, String> {
    let cfg = config_path();
    if cfg.exists() {
        if let Ok(data) = fs::read_to_string(&cfg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(dir) = v.get("output_dir").and_then(|d| d.as_str()) {
                    if Path::new(dir).exists() {
                        return Ok(dir.to_string());
                    }
                }
            }
        }
    }
    let default_dir = "/Users/honamgung/Documents/00_유튜브_영상_다운로드";
    let _ = fs::create_dir_all(default_dir);
    Ok(default_dir.to_string())
}

#[tauri::command]
async fn save_output_dir(dir: String) -> Result<(), String> {
    let cfg = config_path();
    let json = serde_json::json!({ "output_dir": dir });
    fs::write(&cfg, serde_json::to_string_pretty(&json).unwrap())
        .map_err(|e| format!("설정 저장 실패: {}", e))
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    percent: f64,
    status: String,
}

#[tauri::command]
async fn download(
    app: AppHandle,
    url: String,
    mode: String,
    filename: Option<String>,
    output_dir: String,
) -> Result<String, String> {
    let bin = find_ytdlp()?;

    // 폴더명: 사용자 지정 파일명 또는 영상 제목
    let folder_name = if let Some(ref name) = filename {
        let clean: String = name
            .chars()
            .filter(|c| !['/', '\\', ':', '*', '?', '"', '<', '>', '|'].contains(c))
            .collect();
        clean
    } else {
        String::new()
    };

    // 폴더명이 있으면 고정 폴더/파일명, 없으면 yt-dlp가 제목으로 자동 생성
    let output_template = if folder_name.is_empty() {
        format!("{}/%(title)s/%(title)s.%(ext)s", output_dir)
    } else {
        format!("{}/{}/{}.%(ext)s", output_dir, folder_name, folder_name)
    };

    let mut args: Vec<String> = vec![url.clone()];

    match mode.as_str() {
        "audio_wav" => {
            args.extend(["-x", "--audio-format", "wav", "--audio-quality", "0"].map(String::from));
        }
        "audio_mp3" => {
            args.extend(["-x", "--audio-format", "mp3", "--audio-quality", "0"].map(String::from));
        }
        "video_best" => {
            args.extend(
                ["-f", "bestvideo+bestaudio/best", "--merge-output-format", "mp4"].map(String::from),
            );
        }
        "video_1080" => {
            args.extend(
                [
                    "-f",
                    "bestvideo[height<=1080]+bestaudio/best[height<=1080]/best",
                    "--merge-output-format",
                    "mp4",
                ]
                .map(String::from),
            );
        }
        "sub_only" => {
            args.extend(
                ["--write-auto-subs", "--sub-langs", "ko", "--skip-download"].map(String::from),
            );
        }
        "video_sub" => {
            args.extend(
                [
                    "--write-subs",
                    "--write-auto-subs",
                    "--sub-langs",
                    "ko",
                    "-f",
                    "bestvideo+bestaudio/best",
                    "--merge-output-format",
                    "mp4",
                ]
                .map(String::from),
            );
        }
        "video_hardsub" => {
            args.extend(
                [
                    "--write-auto-subs",
                    "--sub-langs",
                    "ko",
                    "--embed-subs",
                    "-f",
                    "bestvideo+bestaudio/best",
                    "--merge-output-format",
                    "mp4",
                ]
                .map(String::from),
            );
        }
        _ => return Err(format!("알 수 없는 모드: {}", mode)),
    }

    args.extend(["-o".to_string(), output_template]);
    args.push("--newline".to_string());
    args.push("--no-colors".to_string());

    // 429 방지: iOS/Android 클라이언트 사용 (웹보다 레이트 리밋이 느슨함)
    args.extend([
        "--extractor-args",
        "youtube:player_client=ios,android,web",
    ].map(String::from));
    // 재시도 줄임 (429 나면 더 반복해도 소용없음)
    args.extend(["--retries", "2"].map(String::from));
    args.extend(["--extractor-retries", "1"].map(String::from));
    args.extend(["--sleep-requests", "1.5"].map(String::from));
    args.extend(["--sleep-subtitles", "3"].map(String::from));

    // Node.js 런타임 자동 탐지 (yt-dlp YouTube 파싱에 필요)
    for node_path in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if std::path::Path::new(node_path).exists() {
            args.push("--js-runtimes".to_string());
            args.push(format!("nodejs:{}", node_path));
            break;
        }
    }

    // ffmpeg 경로 자동 탐지 (.app 번들에서는 PATH가 제한됨)
    for ffmpeg_dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if std::path::Path::new(&format!("{}/ffmpeg", ffmpeg_dir)).exists() {
            args.push("--ffmpeg-location".to_string());
            args.push(ffmpeg_dir.to_string());
            break;
        }
    }

    // 다운로드 전 기존 폴더 목록 기억 (새 폴더에만 webloc 생성용)
    let existing_dirs: std::collections::HashSet<std::path::PathBuf> = fs::read_dir(&output_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.is_dir() { Some(p) } else { None }
                })
                .collect()
        })
        .unwrap_or_default();

    // stderr를 stdout에 합쳐서 진행률 + 경고 모두 캡처
    let mut child = Command::new(&bin)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("yt-dlp 실행 실패: {}", e))?;

    if let Some(pid) = child.id() {
        *CHILD_PID.lock().unwrap() = Some(pid);
    }

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // stdout에서 진행률 파싱
    let app_clone = app.clone();
    let stdout_task = tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        let re = Regex::new(r"\[download\]\s+(\d+\.?\d*)%").unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(caps) = re.captures(&line) {
                if let Ok(pct) = caps[1].parse::<f64>() {
                    let _ = app_clone.emit(
                        "download-progress",
                        ProgressPayload {
                            percent: pct,
                            status: line.clone(),
                        },
                    );
                }
            }
        }
    });

    // stderr 수집 (에러 메시지 확인용)
    let stderr_task = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut err_output = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                err_output.push_str(&line);
                err_output.push('\n');
            }
        }
        err_output
    });

    let _ = stdout_task.await;
    let stderr_output = stderr_task.await.unwrap_or_default();
    let status = child.wait().await.map_err(|e| e.to_string())?;
    *CHILD_PID.lock().unwrap() = None;

    // yt-dlp의 진짜 에러는 "ERROR:" 접두사가 붙음. 그 외(WARNING 등)는 성공 처리
    // 단, 영상 모드에서 자막 관련 에러는 영상이 받아졌으면 무시
    let is_video_mode = matches!(mode.as_str(), "video_sub" | "video_hardsub" | "video_best" | "video_1080");
    let fatal_errors: Vec<&str> = stderr_output
        .lines()
        .filter(|l| l.trim_start().starts_with("ERROR:"))
        .collect();
    let subtitle_only_errors = !fatal_errors.is_empty()
        && fatal_errors.iter().all(|e| {
            e.contains("subtitles")
                || e.contains("No subtitles")
                || e.contains("Unable to download video subtitles")
        });
    // 영상 모드 + 자막 에러만 있으면 진짜 에러 아님
    let is_real_error = !status.success()
        && !fatal_errors.is_empty()
        && !(is_video_mode && subtitle_only_errors);

    if !is_real_error {
        // 새로 생성된 폴더 찾기
        let mut new_folders: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(&output_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !existing_dirs.contains(&path) {
                    new_folders.push(path);
                }
            }
        }

        // 자막 모드면 .vtt 파일이 실제로 생성됐는지 확인
        let has_subs = matches!(mode.as_str(), "sub_only" | "video_sub" | "video_hardsub");
        let mut vtt_found = false;
        if has_subs {
            for folder in &new_folders {
                if let Ok(sub_entries) = fs::read_dir(folder) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if sub_path.extension().map_or(false, |e| e == "vtt") {
                            vtt_found = true;
                            vtt_to_txt(&sub_path);
                        }
                    }
                }
            }
        }

        // sub_only 모드에서 자막 파일이 없으면 실패 처리
        if mode == "sub_only" && !vtt_found {
            return Err("자막 다운로드에 실패했습니다. (자막이 없거나 YouTube 레이트 리밋)".to_string());
        }

        // 링크 바로가기 생성
        for folder in &new_folders {
            create_webloc(folder, &url);
        }

        let _ = app.emit(
            "download-progress",
            ProgressPayload {
                percent: 100.0,
                status: "완료".to_string(),
            },
        );
        Ok("다운로드 완료!".to_string())
    } else {
        // ERROR: 줄만 찾아서 표시 (WARNING은 무시)
        let error_line = stderr_output
            .lines()
            .find(|l| l.trim_start().starts_with("ERROR:"))
            .unwrap_or("다운로드 중 오류가 발생했습니다.");
        Err(error_line.to_string())
    }
}

#[tauri::command]
async fn cancel_download() -> Result<(), String> {
    let pid = CHILD_PID.lock().unwrap().take();
    if let Some(pid) = pid {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        Ok(())
    } else {
        Err("진행 중인 다운로드가 없습니다.".to_string())
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            check_ytdlp,
            fetch_title,
            download,
            cancel_download,
            get_default_download_dir,
            save_output_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
