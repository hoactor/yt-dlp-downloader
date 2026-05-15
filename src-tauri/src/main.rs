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

/// whisper-cli 실행 파일 경로 탐지
fn find_whisper_cli() -> Result<String, String> {
    let candidates = [
        "/opt/homebrew/bin/whisper-cli",
        "/usr/local/bin/whisper-cli",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    std::process::Command::new("which")
        .arg("whisper-cli")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if !p.is_empty() { Some(p) } else { None }
            } else {
                None
            }
        })
        .ok_or_else(|| "whisper-cli가 설치되어 있지 않습니다. brew install whisper-cpp 를 실행하세요.".to_string())
}

/// whisper 모델 파일 경로 탐지 (large-v3 우선)
fn find_whisper_model() -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{}/whisper-models/ggml-large-v3.bin", home),
        format!("{}/whisper-models/ggml-large-v3-turbo.bin", home),
        "/opt/homebrew/share/whisper-cpp/models/ggml-large-v3.bin".to_string(),
        format!("{}/.cache/whisper/ggml-large-v3.bin", home),
    ];
    for path in candidates.iter() {
        if std::path::Path::new(path).exists() {
            return Ok(path.clone());
        }
    }
    Err(format!(
        "whisper 모델 파일을 찾을 수 없습니다.\n\n다음 명령어로 다운로드하세요:\n\nmkdir -p ~/whisper-models && cd ~/whisper-models && curl -L -o ggml-large-v3.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
    ))
}

/// ffmpeg 실행 파일 경로 탐지
fn find_ffmpeg() -> Result<String, String> {
    let candidates = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    Err("ffmpeg가 설치되어 있지 않습니다.".to_string())
}

/// ffprobe로 영상 길이(초) 반환
fn get_media_duration(path: &str) -> Option<f64> {
    let ffprobe = ["/opt/homebrew/bin/ffprobe", "/usr/local/bin/ffprobe"]
        .iter()
        .find(|p| std::path::Path::new(p).exists())?;
    let output = std::process::Command::new(ffprobe)
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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
    let is_live_mode = matches!(mode.as_str(), "live_from_start" | "live_now");

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
                ["--write-subs", "--write-auto-subs", "--sub-langs", "ko,en", "--skip-download"].map(String::from),
            );
        }
        "video_sub" => {
            args.extend(
                [
                    "--write-subs",
                    "--write-auto-subs",
                    "--sub-langs",
                    "ko,en",
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
                    "--write-subs",
                    "--write-auto-subs",
                    "--sub-langs",
                    "ko,en",
                    "--embed-subs",
                    "-f",
                    "bestvideo+bestaudio/best",
                    "--merge-output-format",
                    "mp4",
                ]
                .map(String::from),
            );
        }
        "live_from_start" => {
            // 진행 중인 라이브를 방송 시작 시점부터 받기
            // --hls-use-mpegts: .ts 컨테이너로 저장 → 중간 중단 시에도 재생 가능
            // --wait-for-video: 라이브 메타데이터 안정 대기
            // --fragment-retries 20: 조각 다운로드 실패 시 20번 재시도(기본 10) → 스킵 빈도 감소
            args.extend(
                [
                    "-f",
                    "bv*+ba/b",
                    "--live-from-start",
                    "--hls-use-mpegts",
                    "--wait-for-video",
                    "30",
                    "--fragment-retries",
                    "20",
                    "--merge-output-format",
                    "mp4",
                ]
                .map(String::from),
            );
        }
        "live_now" => {
            // 진행 중인 라이브를 현재 시점부터 받기
            args.extend(
                [
                    "-f",
                    "bv*+ba/b",
                    "--hls-use-mpegts",
                    "--wait-for-video",
                    "30",
                    "--fragment-retries",
                    "20",
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
    // 한 언어 자막이 실패해도 다음 언어 계속 시도 (ko 429 -> en 시도)
    let has_subs_mode = matches!(mode.as_str(), "sub_only" | "video_sub" | "video_hardsub");
    if has_subs_mode {
        args.push("--ignore-errors".to_string());
    }

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
    // 다운로드 시작 시각 (기존 폴더에 다시 받는 경우에도 새로 생성된 vtt를 식별)
    let download_start = std::time::SystemTime::now();

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
    // ffmpeg는 진행률을 \r(캐리지 리턴)로만 갱신해서 lines() 못 잡음
    // → byte 단위로 읽고 \r 또는 \n 둘 다 라인 구분자로 처리
    let app_clone = app.clone();
    let skip_count_clone = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let skip_count_for_task = skip_count_clone.clone();
    let is_live_mode_for_task = is_live_mode;
    let stdout_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut reader = stdout;
        let mut byte_buf = vec![0u8; 4096];
        let mut line_buf = String::new();
        // 일반: [download] 12.3%
        let re_pct = Regex::new(r"\[download\]\s+(\d+\.?\d*)%").unwrap();
        // 라이브 DASH: [download] 1.03GiB at 584KB/s (frag 3062/3062)
        let re_frag = Regex::new(
            r"\[download\]\s+([\d.]+\s*[KMG]?i?B)\s+at\s+[^\(]*\(frag\s+(\d+)/(\??\d*)\)"
        ).unwrap();
        // 라이브 ffmpeg: frame=N fps=F ... size=SIZE time=HH:MM:SS bitrate=BR
        let re_ffmpeg = Regex::new(
            r"frame=\s*\d+.*?size=\s*([\d.]+\s*[KMG]?i?B).*?time=([\d:.]+)"
        ).unwrap();
        // 라이브 yt-dlp 네이티브 HLS 단일 스트림: [download] 8.50MiB at 1.2MiB/s ...
        let re_size = Regex::new(
            r"\[download\]\s+([\d.]+\s*[KMG]?i?B)\s+at\s+[\d.]+\s*[KMG]?i?B/s"
        ).unwrap();
        // 라이브: fragment not found; Skipping fragment N ...
        let re_skip = Regex::new(r"fragment not found; Skipping fragment").unwrap();

        let process_line = |line: &str,
                            skip_count: &std::sync::Arc<std::sync::atomic::AtomicU64>,
                            app: &AppHandle| {
            if is_live_mode_for_task {
                if re_skip.is_match(line) {
                    skip_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                let skipped = skip_count.load(std::sync::atomic::Ordering::Relaxed);
                let status_opt = if let Some(caps) = re_frag.captures(line) {
                    // DASH 분리 스트림 (가장 정보 많음)
                    let size = &caps[1];
                    let frag = &caps[2];
                    Some(if skipped > 0 {
                        format!("녹화 중 · {} 조각 · {} · 누락 {}", frag, size, skipped)
                    } else {
                        format!("녹화 중 · {} 조각 · {}", frag, size)
                    })
                } else if let Some(caps) = re_ffmpeg.captures(line) {
                    // ffmpeg 진행률
                    let size = &caps[1];
                    let time = &caps[2];
                    Some(if skipped > 0 {
                        format!("녹화 중 · {} · 영상 {} · 누락 {}", size, time, skipped)
                    } else {
                        format!("녹화 중 · {} · 영상 {}", size, time)
                    })
                } else if let Some(caps) = re_size.captures(line) {
                    // yt-dlp 네이티브 HLS 단일 스트림
                    let size = &caps[1];
                    Some(if skipped > 0 {
                        format!("녹화 중 · {} · 누락 {}", size, skipped)
                    } else {
                        format!("녹화 중 · {}", size)
                    })
                } else {
                    None
                };
                if let Some(status) = status_opt {
                    let _ = app.emit(
                        "download-progress",
                        ProgressPayload {
                            percent: -1.0,
                            status,
                        },
                    );
                }
            } else if let Some(caps) = re_pct.captures(line) {
                if let Ok(pct) = caps[1].parse::<f64>() {
                    let _ = app.emit(
                        "download-progress",
                        ProgressPayload {
                            percent: pct,
                            status: line.to_string(),
                        },
                    );
                }
            }
        };

        loop {
            match reader.read(&mut byte_buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&byte_buf[..n]);
                    for ch in chunk.chars() {
                        if ch == '\n' || ch == '\r' {
                            if !line_buf.is_empty() {
                                process_line(&line_buf, &skip_count_for_task, &app_clone);
                                line_buf.clear();
                            }
                        } else {
                            line_buf.push(ch);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        // EOF 시 마지막 라인 플러시
        if !line_buf.is_empty() {
            process_line(&line_buf, &skip_count_for_task, &app_clone);
        }
    });

    // stderr 수집 (에러 메시지 확인용)
    let skip_count_for_stderr = skip_count_clone.clone();
    let stderr_task = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut err_output = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                // 라이브 fragment 스킵은 stderr로도 나올 수 있음
                if line.contains("Skipping fragment") {
                    skip_count_for_stderr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
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
    let is_video_mode = matches!(mode.as_str(),
        "video_sub" | "video_hardsub" | "video_best" | "video_1080"
        | "live_from_start" | "live_now");
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
    // 라이브 모드: fragment 가져오기 실패는 회복 가능한 일시 에러로 간주
    let live_recoverable_errors = is_live_mode
        && !fatal_errors.is_empty()
        && fatal_errors.iter().all(|e| {
            e.contains("Did not get any data blocks")
                || e.contains("fragment")
                || e.contains("HTTP Error 5")
        });

    // 새로 생성된 폴더 + 기존 폴더(같은 영상 재시도) 모두 검사 대상
    let mut target_folders: Vec<std::path::PathBuf> = Vec::new();
    let mut new_folders: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if !existing_dirs.contains(&path) {
                    new_folders.push(path.clone());
                }
                target_folders.push(path);
            }
        }
    }

    // 다운로드 시작 이후 생성/수정된 파일만 "이번 다운로드 결과"로 간주
    let mut vtt_found = false;
    let mut video_found = false;
    for folder in &target_folders {
        if let Ok(sub_entries) = fs::read_dir(folder) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                let is_recent = sub_entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t >= download_start)
                    .unwrap_or(false);
                if !is_recent {
                    continue;
                }
                if let Some(ext) = sub_path.extension().and_then(|e| e.to_str()) {
                    match ext {
                        "vtt" => {
                            vtt_found = true;
                            vtt_to_txt(&sub_path);
                        }
                        "mp4" | "mkv" | "webm" | "mov" | "ts" => video_found = true,
                        _ => {}
                    }
                }
            }
        }
    }

    // 영상 모드: 영상 파일이 있으면 자막 에러 무시
    // sub_only 모드: vtt 하나라도 받았으면 성공 (--ignore-errors로 ko 실패 후 en 받은 경우)
    // 라이브 모드: fragment 일시 에러는 회복 가능한 것으로 간주
    let is_real_error = !fatal_errors.is_empty()
        && !(is_video_mode && (video_found || subtitle_only_errors))
        && !(mode == "sub_only" && vtt_found)
        && !live_recoverable_errors;

    if !is_real_error {
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
        // 라이브 모드: 스킵 카운트 반영한 완료 메시지
        if is_live_mode {
            let skipped = skip_count_clone.load(std::sync::atomic::Ordering::Relaxed);
            if skipped == 0 {
                Ok("녹화 완료!".to_string())
            } else {
                Ok(format!("녹화 완료 · 조각 {} 개 누락 (영상 갭 약 {}초)", skipped, skipped * 2))
            }
        } else {
            Ok("다운로드 완료!".to_string())
        }
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

#[derive(Clone, Serialize)]
struct WhisperCheck {
    whisper: bool,
    model: bool,
    ffmpeg: bool,
    model_path: String,
}

/// whisper 환경 체크: 실행 파일 / 모델 / ffmpeg 존재 여부
#[tauri::command]
async fn check_whisper() -> Result<WhisperCheck, String> {
    let whisper = find_whisper_cli().is_ok();
    let (model, model_path) = match find_whisper_model() {
        Ok(p) => (true, p),
        Err(_) => (false, String::new()),
    };
    let ffmpeg = find_ffmpeg().is_ok();
    Ok(WhisperCheck { whisper, model, ffmpeg, model_path })
}

/// 영상/오디오에서 자막 생성 (whisper-cli)
/// 흐름: ffmpeg로 16kHz mono wav 추출 → whisper-cli 실행 → 결과(txt/srt)를 원본 영상 폴더에 저장
#[tauri::command]
async fn transcribe(
    app: AppHandle,
    media_path: String,
    language: String,
) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let whisper_bin = find_whisper_cli()?;
    let model = find_whisper_model()?;
    let ffmpeg_bin = find_ffmpeg()?;

    let media = std::path::Path::new(&media_path);
    if !media.exists() {
        return Err(format!("파일을 찾을 수 없습니다: {}", media_path));
    }

    // 출력 경로: 원본 영상과 같은 폴더, 같은 이름 (확장자만 변경)
    let parent = media.parent()
        .ok_or_else(|| "영상 폴더를 찾을 수 없습니다.".to_string())?;
    let stem = media.file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "파일명을 처리할 수 없습니다.".to_string())?;
    let output_base = parent.join(stem);
    let output_base_str = output_base.to_string_lossy().to_string();

    // 영상 총 길이 (진행률 계산용, 실패해도 진행률 % 표시는 whisper의 progress가 대신 함)
    let duration = get_media_duration(&media_path);

    // 임시 wav 추출 경로 (영상 폴더에 만들고 끝나면 삭제)
    let temp_wav = parent.join(format!(".{}.whisper.wav", stem));
    let temp_wav_str = temp_wav.to_string_lossy().to_string();

    // === 1단계: ffmpeg로 wav 추출 ===
    let _ = app.emit(
        "transcribe-progress",
        ProgressPayload { percent: 0.0, status: "오디오 추출 중...".to_string() },
    );
    let ff_status = Command::new(&ffmpeg_bin)
        .args([
            "-i", &media_path,
            "-vn",
            "-ar", "16000",
            "-ac", "1",
            "-c:a", "pcm_s16le",
            "-y",
            &temp_wav_str,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(|e| format!("ffmpeg 실행 실패: {}", e))?;
    if !ff_status.success() {
        return Err("오디오 추출 실패 (지원되지 않는 포맷일 수 있음)".to_string());
    }

    // === 2단계: whisper-cli 실행 ===
    let _ = app.emit(
        "transcribe-progress",
        ProgressPayload { percent: 0.0, status: "자막 생성 중...".to_string() },
    );
    let mut child = Command::new(&whisper_bin)
        .args([
            "-m", &model,
            "-l", &language,
            "-otxt",
            "-osrt",
            "-pp",
            "-of", &output_base_str,
            &temp_wav_str,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("whisper-cli 실행 실패: {}", e))?;

    if let Some(pid) = child.id() {
        *CHILD_PID.lock().unwrap() = Some(pid);
    }

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let app_for_stdout = app.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = stdout;
        let mut byte_buf = vec![0u8; 4096];
        let mut line_buf = String::new();
        let re_progress = Regex::new(r"progress\s*=\s*(\d+)%").unwrap();
        let re_sub = Regex::new(
            r"\[(\d{2}:\d{2}:\d{2})\.\d+\s+-->\s+\d{2}:\d{2}:\d{2}\.\d+\]\s+(.+)"
        ).unwrap();
        let mut last_sub: String = String::new();

        let mut process_line = |line: &str| {
            if let Some(caps) = re_progress.captures(line) {
                if let Ok(pct) = caps[1].parse::<f64>() {
                    let status = if last_sub.is_empty() {
                        format!("자막 생성 중 · {}%", pct as i32)
                    } else {
                        format!("자막 생성 중 · {}% · {}", pct as i32, last_sub)
                    };
                    let _ = app_for_stdout.emit(
                        "transcribe-progress",
                        ProgressPayload { percent: pct, status },
                    );
                }
            } else if let Some(caps) = re_sub.captures(line) {
                let timestamp = &caps[1];
                let text = caps[2].trim();
                last_sub = format!("[{}] {}", timestamp, text);
                // 영상 총 길이 알면 timestamp 기반 진행률 보조 계산
                let pct_from_ts = if let Some(total) = duration {
                    let parts: Vec<&str> = timestamp.split(':').collect();
                    if parts.len() == 3 {
                        let h: f64 = parts[0].parse().unwrap_or(0.0);
                        let m: f64 = parts[1].parse().unwrap_or(0.0);
                        let s: f64 = parts[2].parse().unwrap_or(0.0);
                        let cur = h * 3600.0 + m * 60.0 + s;
                        if total > 0.0 { Some((cur / total * 100.0).min(99.0)) } else { None }
                    } else { None }
                } else { None };

                let pct = pct_from_ts.unwrap_or(0.0);
                let _ = app_for_stdout.emit(
                    "transcribe-progress",
                    ProgressPayload {
                        percent: pct,
                        status: format!("자막 생성 중 · {}", last_sub),
                    },
                );
            }
        };

        loop {
            match reader.read(&mut byte_buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&byte_buf[..n]);
                    for ch in chunk.chars() {
                        if ch == '\n' || ch == '\r' {
                            if !line_buf.is_empty() {
                                process_line(&line_buf);
                                line_buf.clear();
                            }
                        } else {
                            line_buf.push(ch);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !line_buf.is_empty() {
            process_line(&line_buf);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        let mut err = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                err.push_str(&line);
                err.push('\n');
            }
        }
        err
    });

    let _ = stdout_task.await;
    let stderr_output = stderr_task.await.unwrap_or_default();
    let status = child.wait().await.map_err(|e| e.to_string())?;
    *CHILD_PID.lock().unwrap() = None;

    // 임시 wav 삭제
    let _ = fs::remove_file(&temp_wav);

    if !status.success() {
        // ERROR 줄만 표시
        let err_line = stderr_output
            .lines()
            .find(|l| l.to_lowercase().contains("error"))
            .unwrap_or("자막 생성에 실패했습니다.");
        return Err(err_line.to_string());
    }

    // 결과 파일 존재 확인
    let txt_path = format!("{}.txt", output_base_str);
    if !std::path::Path::new(&txt_path).exists() {
        return Err("자막 파일이 생성되지 않았습니다.".to_string());
    }

    let _ = app.emit(
        "transcribe-progress",
        ProgressPayload { percent: 100.0, status: "완료".to_string() },
    );
    Ok(format!("자막 생성 완료!\n{}.txt\n{}.srt", stem, stem))
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
            check_whisper,
            transcribe,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
