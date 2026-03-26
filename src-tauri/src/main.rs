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

#[tauri::command]
async fn get_default_download_dir() -> Result<String, String> {
    dirs::download_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "다운로드 폴더를 찾을 수 없습니다.".to_string())
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
                ["-f", "bestvideo+bestaudio", "--merge-output-format", "mp4"].map(String::from),
            );
        }
        "video_1080" => {
            args.extend(
                [
                    "-f",
                    "bestvideo[height<=1080]+bestaudio",
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
                    "bestvideo+bestaudio",
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
                    "bestvideo+bestaudio",
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
    let has_fatal_error = stderr_output.lines().any(|l| l.trim_start().starts_with("ERROR:"));
    let is_real_error = !status.success() && has_fatal_error;

    if !is_real_error {
        // 자막이 포함된 모드면 .vtt → .txt 자동 변환
        let has_subs = matches!(mode.as_str(), "sub_only" | "video_sub" | "video_hardsub");
        if has_subs {
            // output_dir 하위에서 .vtt 파일 찾기
            if let Ok(entries) = fs::read_dir(&output_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        // 영상제목 폴더 안에서 .vtt 찾기
                        if let Ok(sub_entries) = fs::read_dir(&path) {
                            for sub_entry in sub_entries.flatten() {
                                let sub_path = sub_entry.path();
                                if sub_path.extension().map_or(false, |e| e == "vtt") {
                                    vtt_to_txt(&sub_path);
                                }
                            }
                        }
                    }
                }
            }
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
        .invoke_handler(tauri::generate_handler![
            check_ytdlp,
            fetch_title,
            download,
            cancel_download,
            get_default_download_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
