#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use regex::Regex;
use serde::Serialize;
use std::process::Stdio;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

static CHILD_PID: Mutex<Option<u32>> = Mutex::new(None);

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

    let output_template = if let Some(ref name) = filename {
        let clean: String = name
            .chars()
            .filter(|c| !['/', '\\', ':', '*', '?', '"', '<', '>', '|'].contains(c))
            .collect();
        format!("{}/{}.%(ext)s", output_dir, clean)
    } else {
        format!("{}/%(title)s.%(ext)s", output_dir)
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
                ["--write-auto-subs", "--sub-langs", "ko", "--embed-subs"].map(String::from),
            );
        }
        _ => return Err(format!("알 수 없는 모드: {}", mode)),
    }

    args.extend(["-o".to_string(), output_template]);
    args.push("--newline".to_string());
    args.push("--no-colors".to_string());

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
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    let re = Regex::new(r"\[download\]\s+(\d+\.?\d*)%").unwrap();

    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(caps) = re.captures(&line) {
            if let Ok(pct) = caps[1].parse::<f64>() {
                let _ = app.emit(
                    "download-progress",
                    ProgressPayload {
                        percent: pct,
                        status: line.clone(),
                    },
                );
            }
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    *CHILD_PID.lock().unwrap() = None;

    if status.success() {
        let _ = app.emit(
            "download-progress",
            ProgressPayload {
                percent: 100.0,
                status: "완료".to_string(),
            },
        );
        Ok("다운로드 완료!".to_string())
    } else {
        Err("다운로드 중 오류가 발생했습니다.".to_string())
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
