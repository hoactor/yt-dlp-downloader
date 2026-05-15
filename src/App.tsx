import { useState, useEffect, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

const APP_VERSION = "1.0.0";
const BUILD_DATE = "2026-03-27";

type Tab = "audio" | "video" | "subtitle" | "live" | "whisper";

const LANG_OPTIONS = [
  { id: "ko", label: "한국어" },
  { id: "en", label: "English" },
  { id: "ja", label: "日本語" },
  { id: "auto", label: "자동 감지" },
];

interface ToastState {
  message: string;
  type: "success" | "error";
}

const AUDIO_OPTIONS = [
  { id: "audio_wav", label: "WAV", desc: "무손실, 편집용" },
  { id: "audio_mp3", label: "MP3", desc: "경량, 공유용" },
];

const VIDEO_OPTIONS = [
  { id: "video_best", label: "최고 화질", desc: "원본 최대 해상도" },
  { id: "video_1080", label: "1080p", desc: "Full HD 고정" },
];

const SUB_OPTIONS = [
  { id: "sub_only", label: "자막만", desc: "자동생성 포함" },
  { id: "video_sub", label: "영상+자막", desc: "별도 자막 파일" },
  { id: "video_hardsub", label: "하드코딩", desc: "자막 내장" },
];

const LIVE_OPTIONS = [
  { id: "live_from_start", label: "처음부터", desc: "방송 시작 시점부터" },
  { id: "live_now", label: "지금부터", desc: "현재 시점부터 녹화" },
];

function App() {
  const [url, setUrl] = useState("");
  const [title, setTitle] = useState("");
  const [filename, setFilename] = useState("");
  const [tab, setTab] = useState<Tab>("video");
  const [selectedMode, setSelectedMode] = useState("video_1080");
  const [outputDir, setOutputDir] = useState("");
  const [ytdlpVersion, setYtdlpVersion] = useState("");
  const [ytdlpMissing, setYtdlpMissing] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [progress, setProgress] = useState(0);
  const [liveStatus, setLiveStatus] = useState("");
  const [elapsedSec, setElapsedSec] = useState(0);
  const [toast, setToast] = useState<ToastState | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  // Whisper 상태
  const [whisperFile, setWhisperFile] = useState("");
  const [whisperLang, setWhisperLang] = useState("ko");
  const [whisperStatus, setWhisperStatus] = useState("");
  const [dragOver, setDragOver] = useState(false);
  const [whisperReady, setWhisperReady] = useState<{ whisper: boolean; model: boolean; ffmpeg: boolean } | null>(null);
  const titleTimeout = useRef<ReturnType<typeof setTimeout> | null>(null);
  const elapsedTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  const isLiveMode = selectedMode === "live_from_start" || selectedMode === "live_now";
  const isWhisperTab = tab === "whisper";

  useEffect(() => {
    invoke<string>("check_ytdlp")
      .then((v) => setYtdlpVersion(v))
      .catch(() => setYtdlpMissing(true));

    invoke<string>("get_default_download_dir")
      .then((dir) => setOutputDir(dir))
      .catch(() => {});

    const unlisten = listen<{ percent: number; status: string }>(
      "download-progress",
      (event) => {
        if (event.payload.percent < 0) {
          // 라이브: percent 대신 status 텍스트 사용
          setLiveStatus(event.payload.status);
        } else {
          setProgress(event.payload.percent);
        }
      }
    );

    const unlistenWhisper = listen<{ percent: number; status: string }>(
      "transcribe-progress",
      (event) => {
        setProgress(event.payload.percent);
        setWhisperStatus(event.payload.status);
      }
    );

    // Tauri v2 정식 drag-drop API
    let unlistenWebviewDrop: (() => void) | null = null;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") {
          setDragOver(true);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          const paths = event.payload.paths;
          if (paths && paths.length > 0) {
            setWhisperFile(paths[0]);
          }
        } else {
          // leave
          setDragOver(false);
        }
      })
      .then((fn) => {
        unlistenWebviewDrop = fn;
      })
      .catch((e) => console.error("drag-drop register failed", e));

    return () => {
      unlisten.then((fn) => fn());
      unlistenWhisper.then((fn) => fn());
      if (unlistenWebviewDrop) unlistenWebviewDrop();
    };
  }, []);

  const showToast = useCallback((message: string, type: "success" | "error") => {
    setToast({ message, type });
    setTimeout(() => setToast(null), 3000);
  }, []);

  const handleUrlChange = (value: string) => {
    setUrl(value);
    setTitle("");
    if (titleTimeout.current) clearTimeout(titleTimeout.current);
    if (value.trim()) {
      titleTimeout.current = setTimeout(() => {
        invoke<string>("fetch_title", { url: value })
          .then((t) => setTitle(t))
          .catch(() => {});
      }, 800);
    }
  };

  const handleTabChange = (newTab: Tab) => {
    setTab(newTab);
    if (newTab === "audio") setSelectedMode("audio_wav");
    else if (newTab === "video") setSelectedMode("video_1080");
    else if (newTab === "live") setSelectedMode("live_from_start");
    else if (newTab === "whisper") {
      setSelectedMode("");
      // 처음 진입 시 환경 체크
      if (!whisperReady) {
        invoke<{ whisper: boolean; model: boolean; ffmpeg: boolean }>("check_whisper")
          .then(setWhisperReady)
          .catch(() => setWhisperReady({ whisper: false, model: false, ffmpeg: false }));
      }
    } else setSelectedMode("sub_only");
  };

  const handleWhisperStart = async () => {
    if (!whisperFile) {
      showToast("영상/오디오 파일을 선택하세요.", "error");
      return;
    }
    setDownloading(true);
    setProgress(0);
    setWhisperStatus("준비 중...");
    try {
      const result = await invoke<string>("transcribe", {
        mediaPath: whisperFile,
        language: whisperLang,
      });
      showToast(result, "success");
      setWhisperFile("");
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setDownloading(false);
      setWhisperStatus("");
    }
  };

  const handleFilePick = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        { name: "영상/오디오", extensions: ["mp4", "mkv", "mov", "webm", "ts", "mp3", "wav", "m4a", "ogg", "flac"] },
      ],
    });
    if (selected) setWhisperFile(selected as string);
  };

  const whisperFileName = whisperFile
    ? whisperFile.split("/").pop() || whisperFile
    : "";

  const formatElapsed = (sec: number) => {
    const h = Math.floor(sec / 3600);
    const m = Math.floor((sec % 3600) / 60);
    const s = sec % 60;
    if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
    return `${m}:${String(s).padStart(2, "0")}`;
  };

  const handleDownload = async () => {
    if (!url.trim()) {
      showToast("URL을 입력하세요.", "error");
      return;
    }
    setDownloading(true);
    setProgress(0);
    setLiveStatus(isLiveMode ? "녹화 준비 중..." : "");
    setElapsedSec(0);
    if (isLiveMode) {
      elapsedTimer.current = setInterval(() => {
        setElapsedSec((s) => s + 1);
      }, 1000);
    }
    try {
      const result = await invoke<string>("download", {
        url: url.trim(),
        mode: selectedMode,
        filename: filename.trim() || null,
        outputDir: outputDir,
      });
      showToast(result, "success");
    } catch (e) {
      showToast(String(e), "error");
    } finally {
      setDownloading(false);
      if (elapsedTimer.current) {
        clearInterval(elapsedTimer.current);
        elapsedTimer.current = null;
      }
    }
  };

  const handleCancel = async () => {
    try {
      await invoke("cancel_download");
      setDownloading(false);
      setProgress(0);
      setLiveStatus("");
      if (elapsedTimer.current) {
        clearInterval(elapsedTimer.current);
        elapsedTimer.current = null;
      }
      showToast(isLiveMode ? "녹화가 중지되었습니다." : "다운로드가 취소되었습니다.", "error");
    } catch (e) {
      showToast(String(e), "error");
    }
  };

  const currentOptions =
    tab === "audio"
      ? AUDIO_OPTIONS
      : tab === "video"
        ? VIDEO_OPTIONS
        : tab === "live"
          ? LIVE_OPTIONS
          : SUB_OPTIONS;

  return (
    <div className="app">
      {toast && <div className={`toast ${toast.type}`}>{toast.message}</div>}

      {/* About 모달 */}
      {showAbout && (
        <div className="modal-overlay" onClick={() => setShowAbout(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <span className="modal-icon">🟠</span>
              <span className="modal-title">yt-dlp 다운로더</span>
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">앱 버전</span>
                <span className="modal-value">v{APP_VERSION}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">빌드 날짜</span>
                <span className="modal-value">{BUILD_DATE}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">yt-dlp</span>
                <span className="modal-value">
                  {ytdlpVersion ? `v${ytdlpVersion}` : "미설치"}
                </span>
              </div>
              <div className="modal-row">
                <span className="modal-label">프레임워크</span>
                <span className="modal-value">Tauri v2 + React</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">저장 경로</span>
                <span className="modal-value mono">{outputDir}</span>
              </div>
              <div className="modal-divider" />
              <div className="modal-features">
                <div className="modal-feature-title">지원 기능</div>
                <div className="modal-feature">오디오 추출 (WAV / MP3)</div>
                <div className="modal-feature">영상 다운로드 (최고화질 / 1080p)</div>
                <div className="modal-feature">자막 (VTT + 텍스트 자동 추출)</div>
                <div className="modal-feature">자막 하드코딩 내장</div>
                <div className="modal-feature">영상별 폴더 자동 생성</div>
              </div>
            </div>
            <button className="modal-close-btn" onClick={() => setShowAbout(false)}>
              닫기
            </button>
          </div>
        </div>
      )}

      <div className="header" onClick={() => setShowAbout(true)} style={{ cursor: "pointer" }}>
        <span className="header-icon">🟠</span>
        <h1>yt-dlp 다운로더</h1>
        <span className="header-version">v{APP_VERSION}</span>
      </div>

      {ytdlpMissing && (
        <div className="warning-banner">
          yt-dlp가 설치되어 있지 않습니다. 터미널에서 <code>brew install yt-dlp</code> 를
          실행하세요.
        </div>
      )}

      {!isWhisperTab && (
        <div className="input-group">
          <input
            className="input-field"
            placeholder="YouTube URL을 붙여넣으세요"
            value={url}
            onChange={(e) => handleUrlChange(e.target.value)}
            disabled={downloading}
          />
          <div className="title-preview">{title}</div>
          <input
            className="input-field"
            placeholder="파일명 (비우면 원본 제목 사용)"
            value={filename}
            onChange={(e) => setFilename(e.target.value)}
            disabled={downloading}
          />
        </div>
      )}

      <div className="tabs">
        {(["video", "subtitle", "audio", "live", "whisper"] as Tab[]).map((t) => (
          <button
            key={t}
            className={`tab ${tab === t ? "active" : ""} ${t === "live" ? "tab-live" : ""} ${t === "whisper" ? "tab-whisper" : ""}`}
            onClick={() => handleTabChange(t)}
            disabled={downloading}
          >
            {t === "audio"
              ? "오디오"
              : t === "video"
                ? "영상"
                : t === "live"
                  ? "🔴 라이브"
                  : t === "whisper"
                    ? "🎤 자막생성"
                    : "자막"}
          </button>
        ))}
      </div>

      {isWhisperTab ? (
        <div className="whisper-panel">
          {whisperReady && (!whisperReady.whisper || !whisperReady.model || !whisperReady.ffmpeg) && (
            <div className="warning-banner">
              {!whisperReady.whisper && (
                <div>whisper-cli 미설치. <code>brew install whisper-cpp</code></div>
              )}
              {!whisperReady.model && (
                <div>모델 파일 없음. <code>mkdir -p ~/whisper-models && cd ~/whisper-models && curl -L -o ggml-large-v3.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin</code></div>
              )}
              {!whisperReady.ffmpeg && <div>ffmpeg 미설치. <code>brew install ffmpeg</code></div>}
            </div>
          )}

          <div
            className={`drop-zone ${dragOver ? "drag-over" : ""} ${whisperFile ? "has-file" : ""}`}
            onClick={() => !downloading && handleFilePick()}
          >
            {whisperFile ? (
              <>
                <div className="drop-zone-icon">📄</div>
                <div className="drop-zone-filename">{whisperFileName}</div>
                <div className="drop-zone-hint">클릭해서 다른 파일 선택</div>
              </>
            ) : (
              <>
                <div className="drop-zone-icon">📥</div>
                <div className="drop-zone-text">
                  {dragOver ? "여기에 놓으세요" : "영상 파일을 끌어다 놓으세요"}
                </div>
                <div className="drop-zone-hint">또는 클릭해서 파일 선택</div>
              </>
            )}
          </div>

          <div className="whisper-options">
            <label className="whisper-label">언어</label>
            <select
              className="whisper-select"
              value={whisperLang}
              onChange={(e) => setWhisperLang(e.target.value)}
              disabled={downloading}
            >
              {LANG_OPTIONS.map((l) => (
                <option key={l.id} value={l.id}>{l.label}</option>
              ))}
            </select>
          </div>
        </div>
      ) : (
        <div className={`options-grid ${tab === "subtitle" ? "three-col" : ""}`}>
          {currentOptions.map((opt) => (
            <div
              key={opt.id}
              className={`option-card ${selectedMode === opt.id ? "selected" : ""}`}
              onClick={() => !downloading && setSelectedMode(opt.id)}
            >
              <div className="label">{opt.label}</div>
              <div className="desc">{opt.desc}</div>
            </div>
          ))}
        </div>
      )}

      {downloading ? (
        <>
          {isLiveMode ? (
            <div className="live-status">
              <div className="live-indicator">
                <span className="live-dot" />
                <span className="live-label">{liveStatus || "녹화 준비 중..."}</span>
              </div>
              <div className="live-elapsed">경과 {formatElapsed(elapsedSec)}</div>
            </div>
          ) : isWhisperTab ? (
            <div className="progress-container">
              <div className="progress-bar-bg">
                <div
                  className={`progress-bar-fill ${progress >= 100 ? "done" : ""}`}
                  style={{ width: `${progress}%` }}
                />
              </div>
              <div className="progress-text">{whisperStatus || `${progress.toFixed(0)}%`}</div>
            </div>
          ) : (
            <div className="progress-container">
              <div className="progress-bar-bg">
                <div
                  className={`progress-bar-fill ${progress >= 100 ? "done" : ""}`}
                  style={{ width: `${progress}%` }}
                />
              </div>
              <div className="progress-text">{progress.toFixed(1)}%</div>
            </div>
          )}
          <button className="download-btn cancel" onClick={handleCancel}>
            {isLiveMode ? "녹화 중지" : "취소"}
          </button>
        </>
      ) : isWhisperTab ? (
        <button
          className="download-btn"
          onClick={handleWhisperStart}
          disabled={
            !whisperFile ||
            !!(whisperReady && (!whisperReady.whisper || !whisperReady.model || !whisperReady.ffmpeg))
          }
        >
          🎤 자막 생성
        </button>
      ) : (
        <button
          className="download-btn"
          onClick={handleDownload}
          disabled={ytdlpMissing || !url.trim()}
        >
          {isLiveMode ? "🔴 녹화 시작" : "다운로드"}
        </button>
      )}

      <div className="footer">
        <div className="path-row">
          <span className="path-label">저장 경로:</span>
          <span className="path-value">{outputDir}</span>
          <button
            className="path-change-btn"
            onClick={async () => {
              const selected = await open({ directory: true, defaultPath: outputDir });
              if (selected) {
                setOutputDir(selected as string);
                await invoke("save_output_dir", { dir: selected as string });
              }
            }}
          >
            변경
          </button>
        </div>
        {ytdlpVersion && <div className="version-text">yt-dlp v{ytdlpVersion}</div>}
      </div>
    </div>
  );
}

export default App;
